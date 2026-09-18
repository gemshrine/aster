//! What a call's media path actually is, see `specs/0015-call-diagnostics.md`.
//!
//! Without this a failed call is just `Failed`: no way to tell whether TURN
//! never answered, the peer's candidates never arrived, or the pair check
//! failed.

use rtc::peer_connection::transport::RTCIceCandidateType;
use serde::Serialize;
use webrtc::peer_connection::RTCStatsReport;
use webrtc::peer_connection::RTCStatsReportEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Host,
    Srflx,
    Prflx,
    Relay,
}

impl CandidateKind {
    /// The local candidates WebRTC hands us carry their type already.
    pub fn from_type(typ: RTCIceCandidateType) -> Option<Self> {
        match typ {
            RTCIceCandidateType::Host => Some(Self::Host),
            RTCIceCandidateType::Srflx => Some(Self::Srflx),
            RTCIceCandidateType::Prflx => Some(Self::Prflx),
            RTCIceCandidateType::Relay => Some(Self::Relay),
            RTCIceCandidateType::Unspecified => None,
        }
    }

    /// The peer's candidates arrive as SDP lines: `candidate:… typ srflx …`.
    pub fn from_sdp(candidate: &str) -> Option<Self> {
        let mut fields = candidate.split_whitespace();
        let typ = loop {
            if fields.next()? == "typ" {
                break fields.next()?;
            }
        };
        match typ {
            "host" => Some(Self::Host),
            "srflx" => Some(Self::Srflx),
            "prflx" => Some(Self::Prflx),
            "relay" => Some(Self::Relay),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CandidateCounts {
    pub host: u32,
    pub srflx: u32,
    pub prflx: u32,
    pub relay: u32,
}

impl CandidateCounts {
    pub fn add(&mut self, kind: CandidateKind) {
        let count = match kind {
            CandidateKind::Host => &mut self.host,
            CandidateKind::Srflx => &mut self.srflx,
            CandidateKind::Prflx => &mut self.prflx,
            CandidateKind::Relay => &mut self.relay,
        };
        *count += 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportPath {
    /// No nominated pair yet.
    Unknown,
    Direct,
    /// Media goes through the TURN server.
    Relay,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelectedPair {
    pub local: CandidateKind,
    /// `None` when the report has no entry for the peer's candidate, which
    /// happens for a peer-reflexive one.
    pub remote: Option<CandidateKind>,
    /// `udp` or `tcp` of the local candidate.
    pub protocol: String,
    pub rtt_ms: Option<u32>,
}

impl SelectedPair {
    /// As seen from this client: a relay on either known side means the media
    /// goes through the TURN server.
    pub fn path(&self) -> TransportPath {
        if self.local == CandidateKind::Relay || self.remote == Some(CandidateKind::Relay) {
            TransportPath::Relay
        } else {
            TransportPath::Direct
        }
    }
}

/// Keeps the last few errors; a TURN that never answers repeats forever.
const MAX_ERRORS: usize = 5;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TransportStatus {
    /// ICE connection state as WebRTC words it: `new`, `checking`, …
    pub ice: Option<String>,
    pub gathering: Option<String>,
    pub path: Option<TransportPath>,
    pub selected: Option<SelectedPair>,
    pub local_candidates: CandidateCounts,
    pub remote_candidates: CandidateCounts,
    pub errors: Vec<String>,
}

impl TransportStatus {
    pub fn add_error(&mut self, error: String) {
        if self.errors.contains(&error) {
            return;
        }
        if self.errors.len() == MAX_ERRORS {
            self.errors.remove(0);
        }
        self.errors.push(error);
    }

    pub fn set_selected(&mut self, selected: Option<SelectedPair>) {
        self.path = Some(match &selected {
            Some(pair) => pair.path(),
            None => TransportPath::Unknown,
        });
        self.selected = selected;
    }

    /// One line for the log: the whole picture without digging through JSON.
    pub fn summary(&self) -> String {
        let state = self.ice.as_deref().unwrap_or("-");
        let gathering = self.gathering.as_deref().unwrap_or("-");
        let local = self.local_candidates;
        let remote = self.remote_candidates;
        let path = match &self.selected {
            Some(pair) => format!(
                "{:?} {}/{:?}->{} {} ms",
                pair.path(),
                pair.protocol,
                pair.local,
                pair.remote
                    .map_or("unknown".to_string(), |kind| format!("{kind:?}")),
                pair.rtt_ms.unwrap_or(0)
            ),
            None => "no nominated pair".to_string(),
        };
        format!(
            "ice={state} gathering={gathering} \
             local(host={} srflx={} relay={}) remote(host={} srflx={} relay={}) {path}{}",
            local.host,
            local.srflx,
            local.relay,
            remote.host,
            remote.srflx,
            remote.relay,
            if self.errors.is_empty() {
                String::new()
            } else {
                format!(" errors={:?}", self.errors)
            }
        )
    }
}

/// The pair a call is actually using: the nominated one, or — when the agent
/// has not marked one, as happens on the controlling side — the succeeded pair
/// that carries the most traffic.
pub fn selected_pair(report: &RTCStatsReport) -> Option<SelectedPair> {
    let succeeded = || {
        report
            .candidate_pairs()
            .filter(|pair| pair.state == SUCCEEDED)
    };
    let pair = succeeded()
        .find(|pair| pair.nominated)
        .or_else(|| succeeded().max_by_key(|pair| pair.bytes_sent + pair.bytes_received))?;
    let local = candidate(report, &pair.local_candidate_id)?;
    let remote = candidate(report, &pair.remote_candidate_id);
    let rtt = pair.current_round_trip_time;
    Some(SelectedPair {
        local: CandidateKind::from_type(local.0)?,
        remote: remote.and_then(|remote| CandidateKind::from_type(remote.0)),
        protocol: local.1,
        // Reported in seconds; before the first response it is zero.
        rtt_ms: (rtt > 0.0).then(|| (rtt * 1000.0).round() as u32),
    })
}

use rtc::statistics::stats::ice_candidate_pair::RTCStatsIceCandidatePairState as PairState;
const SUCCEEDED: PairState = PairState::Succeeded;

/// A pair names candidates by their bare id, while the candidate entries are
/// keyed with a prefix (`RTCLocalIceCandidate_<id>`), so the lookup matches on
/// the ending rather than the whole key.
fn candidate(report: &RTCStatsReport, id: &str) -> Option<(RTCIceCandidateType, String)> {
    report.iter().find_map(|entry| match entry {
        RTCStatsReportEntry::LocalCandidate(stats)
        | RTCStatsReportEntry::RemoteCandidate(stats)
            if stats.stats.id.ends_with(id) =>
        {
            Some((stats.candidate_type, stats.protocol.clone()))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_type_of_an_sdp_candidate() {
        let host = "candidate:1 1 udp 2130706431 192.168.1.10 54321 typ host";
        let relay = "candidate:4 1 udp 41885439 203.0.113.7 49170 typ relay raddr 0.0.0.0 rport 0";
        assert_eq!(CandidateKind::from_sdp(host), Some(CandidateKind::Host));
        assert_eq!(CandidateKind::from_sdp(relay), Some(CandidateKind::Relay));
        assert_eq!(
            CandidateKind::from_sdp("candidate:1 1 udp 2130706431"),
            None
        );
        assert_eq!(CandidateKind::from_sdp(""), None);
    }

    #[test]
    fn relay_on_either_side_means_the_call_goes_through_the_server() {
        let pair = |local, remote| SelectedPair {
            local,
            remote: Some(remote),
            protocol: "udp".into(),
            rtt_ms: Some(12),
        };
        assert_eq!(
            pair(CandidateKind::Srflx, CandidateKind::Srflx).path(),
            TransportPath::Direct
        );
        assert_eq!(
            pair(CandidateKind::Host, CandidateKind::Relay).path(),
            TransportPath::Relay
        );
        assert_eq!(
            pair(CandidateKind::Relay, CandidateKind::Host).path(),
            TransportPath::Relay
        );
        // The peer's candidate is not always in the report.
        assert_eq!(
            SelectedPair {
                local: CandidateKind::Srflx,
                remote: None,
                protocol: "udp".into(),
                rtt_ms: None,
            }
            .path(),
            TransportPath::Direct
        );
    }

    #[test]
    fn errors_are_deduplicated_and_capped() {
        let mut status = TransportStatus::default();
        for _ in 0..3 {
            status.add_error("turn: 401 Unauthorized".into());
        }
        assert_eq!(status.errors.len(), 1);
        for n in 0..MAX_ERRORS {
            status.add_error(format!("error {n}"));
        }
        assert_eq!(status.errors.len(), MAX_ERRORS);
        assert_eq!(status.errors[0], "error 0", "the oldest one is dropped");
    }

    #[test]
    fn summary_names_the_path_and_counts() {
        let mut status = TransportStatus {
            ice: Some("connected".into()),
            gathering: Some("complete".into()),
            ..TransportStatus::default()
        };
        status.local_candidates.add(CandidateKind::Host);
        status.local_candidates.add(CandidateKind::Relay);
        status.remote_candidates.add(CandidateKind::Srflx);
        status.set_selected(Some(SelectedPair {
            local: CandidateKind::Relay,
            remote: Some(CandidateKind::Srflx),
            protocol: "tcp".into(),
            rtt_ms: Some(48),
        }));

        let summary = status.summary();
        assert!(summary.contains("ice=connected"), "{summary}");
        assert!(
            summary.contains("local(host=1 srflx=0 relay=1)"),
            "{summary}"
        );
        assert!(
            summary.contains("Relay tcp/Relay->Srflx 48 ms"),
            "{summary}"
        );
        assert_eq!(status.path, Some(TransportPath::Relay));
    }

    #[test]
    fn no_nominated_pair_reads_as_unknown() {
        let mut status = TransportStatus::default();
        status.set_selected(None);
        assert_eq!(status.path, Some(TransportPath::Unknown));
        assert!(status.summary().contains("no nominated pair"));
    }
}
