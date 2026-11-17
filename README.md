# streamer

Rust workspace for the workloads that turn a recorded lesson into a live stream.

| Crate | What it is |
|---|---|
| `stream-common` | what the workloads share: the site API's worker types and client, opening a sealed stream key, redaction of secrets, the destination map and the failure codes |
| `stream-runner` | the binary: polls the site for the next stream due and copies a rendered MP4 to a platform's live ingest with ffmpeg |

The runner has no knowledge of any one site. It speaks a small HTTP protocol with an API at a base
URL you give it, and it needs `ffmpeg` and `ffprobe`.

## How the runner works

```
loop:
  POST {api}/lms/stream_worker/stream/claim            204: nothing due, wait and ask again
  open the stream key (RSA-OAEP, SHA-256, label = destination id) with the private key file
  check the key's characters, map the platform to its fixed ingest address
  ffprobe the presigned MP4 address: H.264 + AAC, at most 1280x720, not longer than the request + 5 s
  ffmpeg ... -c copy -f flv <ingest>/<key>             while:
      POST {api}/lms/stream_worker/stream/{id}/heartbeat   every 15 s, with the position; {"stop": true} ends it
  POST {api}/lms/stream_worker/stream/{id}/finish      no failure_code = the whole file was streamed
```

All calls carry `Authorization: Bearer <runner token>`. Request and response bodies:

| Route | Request | Response |
|---|---|---|
| `POST .../stream/claim` | `{"worker_id"}` | 200 `{"request_id","platform","destination_id","key_sealed","mp4_url","duration_ms","lease_until"}`, 204 nothing waits, 503 try again |
| `POST .../stream/{id}/heartbeat` | `{"worker_id","position_ms"}` | `{"lease_until","stop"}` |
| `POST .../stream/{id}/finish` | `{"worker_id","failure_code"?,"streamed_ms"}` | `{"status":"OK"}` |

- `platform` is an enum (`YOUTUBE`); the ingest address is a constant in `stream-common`
  (`rtmps://a.rtmps.youtube.com/live2`), so no address ever comes from the API or a browser.
- `key_sealed` is the stream key sealed in the browser to the runner's public key (WebCrypto RSA-OAEP,
  SHA-256) with the destination id as the label, base64. Only the runner's private key opens it.
- ffmpeg is run as
  `ffmpeg -nostdin -loglevel error -progress pipe:1 -protocol_whitelist https,tls,tcp -reconnect 1
  -reconnect_on_network_error 1 -reconnect_delay_max 30 -readrate 1 -readrate_initial_burst 2
  [-ss <resume position>] -i <mp4 url> -map 0:v:0 -map 0:a:0 -c copy -rw_timeout 15000000 -f flv
  -flvflags no_duration_filesize <ingest>/<key>`. The position comes from `-progress`'s `out_time_us`.
- After an ffmpeg failure the runner resumes up to twice within three minutes (`-ss` the last
  position), then reports a failure. A stop from the heartbeat sends ffmpeg SIGINT and finishes without a
  failure code. With no successful heartbeat for 45 s the runner stops streaming (the site gives the
  request up by then). SIGTERM stops ffmpeg the same way and reports `stream_failed`.
- The failure codes sent are only `key_rejected`, `connection_lost`, `render_failed` (a file the
  probe refuses), `file_missing` and `stream_failed`. Tool output is never sent.

## Secrets

- The key is checked (printable ASCII, no `/ @ : ? # % \`, quotes or whitespace) before it is used.
- Every line the runner logs goes through a redactor that replaces the stream key and the presigned
  address's query string with `[redacted]`; panics are routed through it too. ffmpeg's stderr is
  redacted before it is logged and is only read for words that tell failures apart.
- **The key is in ffmpeg's argument list while a stream runs.** ffmpeg takes its output address as an
  argument and has no option that reads it from a file, the environment or a pipe (`-/option file`
  covers ffmpeg's own options, not protocol options such as `rtmp_playpath`). Run the runner in a
  container with its own PID namespace, a read-only root filesystem and no extra capabilities, so
  nothing else can read the argument list.

## Configuration

| Variable | Default | |
|---|---|---|
| `STREAM_API_BASE_URL` | required | API root, for example `https://example.org/api/v4`; `https://` only (plain `http://` for a server on the same machine) |
| `STREAM_RUNNER_TOKEN_FILE` | `/secrets/runner-token` | the bearer token |
| `STREAM_RUNNER_KEY_FILE` | `/secrets/runner-key.pem` | the RSA private key, unencrypted PKCS#8 PEM (`openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072`) |
| `STREAM_POLL_INTERVAL_SECS` | `5` | wait between claims when nothing is due |
| `STREAM_WORKER_ID` | `stream-runner` | names this runner to the API (letters, digits, `-_.`, at most 64) |
| `STREAM_FFMPEG`, `STREAM_FFPROBE` | `ffmpeg`, `ffprobe` | paths |
| `RUST_LOG` | `info` | log filter |

The runner refuses to start (exit status 2) when a file is missing or empty, the key is not usable
or the address is not acceptable. Secrets are files so they never sit in the environment.

## Build and test

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked          # no network, no real ffmpeg
cargo audit --deny warnings && cargo deny check
cargo build --release --locked -p stream-runner
```

The toolchain is pinned in `rust-toolchain.toml`; dependencies are pinned to exact versions and
`Cargo.lock` is committed. The tests run the whole loop against a fake API and fake `ffmpeg` and
`ffprobe` shell scripts (success, stop by heartbeat, failure then resume, a file the probe refuses,
a refused key, a lost lease, shutdown), and check that the key and the presigned signature appear in no
log line and no request body. They need a POSIX `sh` and nothing else.

What the tests do not cover: a real ffmpeg copying to a real ingest.

## Licence

MIT or Apache-2.0, at your option.
