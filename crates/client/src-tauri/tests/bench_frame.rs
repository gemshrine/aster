//! Per-frame cost of the audio path in the profile this build uses.
//!
//! Each 20 ms frame must cost far less than 20 ms of CPU, or the call
//! stutters — especially with two clients on one machine (#83).

use std::time::Instant;

use client_tauri_lib::core::voice::media::signal::Sine;
use client_tauri_lib::core::voice::media::{Decoder, Encoder};

#[test]
fn encode_decode_stays_far_below_real_time() {
    let (mut encoder, mut decoder) = (Encoder::new(), Decoder::new());
    let mut sine = Sine::new(440.0, 0.3);

    // Warm up: the first frames pay for allocation and cache misses.
    for _ in 0..20 {
        let packet = encoder.encode(&sine.next_frame());
        decoder.decode(&packet);
    }

    let frames = 500;
    let start = Instant::now();
    for _ in 0..frames {
        let packet = encoder.encode(&sine.next_frame());
        decoder.decode(&packet);
    }
    let per_frame_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
    println!("encode+decode: {per_frame_ms:.2} ms per 20 ms frame");

    // A quarter of the frame is already uncomfortable: the machine also runs
    // the DSP chain, WebRTC and a webview, and may run a second client.
    assert!(
        per_frame_ms < 5.0,
        "audio path costs {per_frame_ms:.2} ms per 20 ms frame"
    );
}
