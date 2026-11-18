//! The ffmpeg command and what it reports.
//!
//! ffmpeg takes its output address as a command-line argument, so the stream key is in the argument
//! list of the ffmpeg process while it runs. No ffmpeg option reads an output address from a file,
//! the environment or a pipe (checked against ffmpeg 9: `-/opt file` covers ffmpeg's own options, not
//! protocol options such as `rtmp_playpath`; `pipe:` outputs would need an RTMP client of our own).
//! What protects the key is the container: its own PID namespace holds only the runner and ffmpeg,
//! so `/proc` shows the arguments to no one else, and the root filesystem is read-only. See the
//! deployment README.

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::process::Child;

/// How long a read or a write may block before it fails, in microseconds (ffmpeg's `-rw_timeout`).
/// It is given before the input and before the output, so it covers both ends.
pub const IO_TIMEOUT_MICROS: &str = "15000000";

/// TLS connections (the file's `https` address, an `rtmps` ingest) check the peer's certificate and
/// name against the system's certificate authorities. ffmpeg checks nothing by default, so without
/// this a network position between this host and a platform could read the stream key. It is given
/// before the input and before the output, and in front of ffprobe's input, so every TLS connection
/// is covered (a plain `rtmp` connection has none to check).
pub const TLS_VERIFY: [&str; 4] = [
    "-tls_verify",
    "1",
    "-ca_file",
    "/etc/ssl/certs/ca-certificates.crt",
];

/// The input is pinned to the one container the render makes: ffmpeg and ffprobe read it as MP4 and
/// will not switch to another demuxer, whatever the file claims to be.
pub const INPUT_FORMAT: [&str; 4] = ["-f", "mp4", "-format_whitelist", "mov,mp4,m4a,3gp,3g2,mj2"];

/// The arguments for one run: copy the file's video and audio to the output, at the file's own pace,
/// starting `start_ms` into it (a resume). `mp4_url` and `output_url` are secrets.
pub fn build_args(mp4_url: &str, output_url: &str, start_ms: i64) -> Vec<String> {
    let mut args: Vec<String> = [
        "-nostdin",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-protocol_whitelist",
        stream_common::media::INPUT_PROTOCOLS,
        "-reconnect",
        "1",
        "-reconnect_on_network_error",
        "1",
        "-reconnect_delay_max",
        "30",
        "-readrate",
        "1",
        "-readrate_initial_burst",
        "2",
        "-rw_timeout",
        IO_TIMEOUT_MICROS,
    ]
    .map(String::from)
    .to_vec();
    args.extend(TLS_VERIFY.map(String::from));
    args.extend(INPUT_FORMAT.map(String::from));
    if start_ms > 0 {
        args.extend([
            "-ss".to_owned(),
            format!("{}.{:03}", start_ms / 1000, start_ms % 1000),
        ]);
    }
    args.extend(["-i".to_owned(), mp4_url.to_owned()]);
    args.extend(
        [
            "-map",
            "0:v:0",
            "-map",
            "0:a:0",
            "-c",
            "copy",
            "-rw_timeout",
            IO_TIMEOUT_MICROS,
        ]
        .map(String::from),
    );
    args.extend(TLS_VERIFY.map(String::from));
    args.extend(["-f", "flv", "-flvflags", "no_duration_filesize"].map(String::from));
    args.push(output_url.to_owned());
    args
}

/// The position in milliseconds a `-progress` line reports, if it is a position line.
pub fn parse_progress_line(line: &str) -> Option<i64> {
    let micros: i64 = line.strip_prefix("out_time_us=")?.trim().parse().ok()?;
    (micros >= 0).then_some(micros / 1000)
}

/// Asks ffmpeg to finish what it is doing and exit (SIGINT), so the output is closed properly.
pub fn interrupt(child: &Child) {
    if let Some(pid) = child.id().and_then(|p| i32::try_from(p).ok()) {
        // The process may have exited a moment ago; there is nothing to do about that.
        let _ = kill(Pid::from_raw(pid), Signal::SIGINT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_the_documented_one() {
        let args = build_args(
            "https://store.example/f.mp4?sig=1",
            "rtmps://ingest.example/live/KEY",
            0,
        )
        .join(" ");
        let protocols = stream_common::media::INPUT_PROTOCOLS;
        assert_eq!(
            args,
            format!(
                "-nostdin -loglevel error -progress pipe:1 -protocol_whitelist {protocols} -reconnect 1 \
                 -reconnect_on_network_error 1 -reconnect_delay_max 30 -readrate 1 -readrate_initial_burst 2 \
                 -rw_timeout 15000000 -tls_verify 1 -ca_file /etc/ssl/certs/ca-certificates.crt -f mp4 -format_whitelist mov,mp4,m4a,3gp,3g2,mj2 \
                 -i https://store.example/f.mp4?sig=1 -map 0:v:0 -map 0:a:0 -c copy -rw_timeout 15000000 -tls_verify 1 -ca_file /etc/ssl/certs/ca-certificates.crt \
                 -f flv -flvflags no_duration_filesize rtmps://ingest.example/live/KEY"
            )
        );
    }

    #[test]
    fn a_resume_seeks_before_the_input() {
        let args = build_args("https://u", "rtmps://o", 83_042);
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        assert_eq!(args[ss + 1], "83.042");
        assert!(ss < args.iter().position(|a| a == "-i").unwrap());
        assert_eq!(build_args("https://u", "rtmps://o", 5_000)[ss + 1], "5.000");
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=1234567"), Some(1234));
        assert_eq!(parse_progress_line("out_time_us=0"), Some(0));
        for other in [
            "out_time_us=N/A",
            "out_time_us=-9",
            "out_time=00:00:01.2",
            "progress=end",
            "",
            "out_time_us=",
        ] {
            assert_eq!(parse_progress_line(other), None, "{other:?}");
        }
    }
}
