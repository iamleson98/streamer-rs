# streamer

Rust workspace for the workloads that turn a recorded lesson into a live stream.

| Crate | What it is |
|---|---|
| `stream-common` | what the workloads share: the site API's worker types and client, opening a sealed stream key, redaction of secrets, the destination map and the failure codes |
| `replay-model` | a recorded lesson's board without drawing: log records, element validation, scene merge, replay clock, pointer previews, camera (a port of the web player's rules) |
| `board-draw` | draws that board natively (Skia): a small `Canvas` trait, the renderer, and the parity check against the web player (`crates/board-draw/README.md`); not used by the released images yet |
| `stream-runner` | the binary: polls the site for the next stream due and copies a rendered MP4 to a platform's live ingest with ffmpeg |
| `stream-render` | the binary: polls the site for a recorded lesson to render, draws it natively (Skia; ffmpeg as child processes to decode, encode and mix) and uploads the MP4 (see "The render worker") |

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
| `POST .../stream/{id}/release` | `{"worker_id"}` | 204 given back; 409 not held, or already streamed |

- `platform` is an enum (`YOUTUBE`, `FACEBOOK`, `TIKTOK`). For YouTube (`rtmps://a.rtmps.youtube.com/live2`)
  and Facebook (`rtmps://live-api-s.facebook.com:443/rtmp`) the ingest address is a constant in
  `stream-common`, so no address ever comes from the API or a browser. TikTok shows a server address with
  every live session, so for TikTok the sealed secret is the whole push address (`rtmp(s)://host/path/key`);
  the runner accepts it only on TikTok's own hosts (`TIKTOK_HOSTS`: `tiktok.com`, `tiktokcdn.com`,
  `tiktokcdn-us.com`, `tiktokv.com` and their subdomains), over `rtmp` or `rtmps`, with no userinfo, query
  in the server part, odd characters or any port but the protocol's own (1935 for `rtmp`, 443 for
  `rtmps`), so a key can never be sent to another host. TikTok hands out plain `rtmp` addresses, and the
  runner accepts them.
- `key_sealed` is the secret sealed in the browser to the runner's public key (WebCrypto RSA-OAEP,
  SHA-256) with the destination id as the label, base64: a stream key (YouTube, Facebook) or the push
  address (TikTok), at most 300 bytes (the runner's RSA key must be 3072 bits or more for a TikTok address). Only the runner's private key opens it.
- ffmpeg is run as
  `ffmpeg -nostdin -loglevel error -progress pipe:1 -protocol_whitelist https,tls,tcp -reconnect 1
  -reconnect_on_network_error 1 -reconnect_delay_max 30 -readrate 1 -readrate_initial_burst 2
  -rw_timeout 15000000 -f mp4 -format_whitelist mov,mp4,m4a,3gp,3g2,mj2
  [-ss <resume position>] -i <mp4 url> -map 0:v:0 -map 0:a:0 -c copy -rw_timeout 15000000 -f flv
  -flvflags no_duration_filesize <ingest>/<key>`. The position comes from `-progress`'s `out_time_us`.
  ffprobe is given the same input format. The file's address must be `https://` on a host in
  `STREAM_MEDIA_HOST`; any other is refused before anything is opened.
- After an ffmpeg failure the runner resumes up to twice within three minutes (`-ss` the last
  position), then reports a failure. A stop from the heartbeat sends ffmpeg SIGINT and finishes without a
  failure code. The runner stops streaming at the lease the site last answered with, less what a stop
  needs (a request in flight plus ffmpeg's grace), or 45 s after the last good heartbeat when the lease
  is not usable; the site gives the request up by then, and nothing is reported. ffmpeg is also stopped
  and resumed when its position does not move for `STREAM_STALL_TIMEOUT_SECS` or one run outlasts the
  rest of the file by a minute. A run that exits cleanly before the end of the file is a failure to
  resume from, not a success. SIGTERM stops ffmpeg the same way and makes one short `stream_failed`
  report; a stream that was claimed but not started is given back with `release` instead. A heartbeat
  answered 404 or 409 ends the stream without a report: the site already knows.
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
| `STREAM_MEDIA_HOST` | required | the host names (comma-separated) the rendered file's address may have |
| `STREAM_STALL_TIMEOUT_SECS` | `60` | ffmpeg is stopped when its position has not moved for this long (10 to 3600) |
| `STREAM_FFMPEG`, `STREAM_FFPROBE` | `ffmpeg`, `ffprobe` | paths |
| `RUST_LOG` | `info` | log filter |

The runner refuses to start (exit status 2) when a file is missing or empty, the key is not usable
or the address is not acceptable. Secrets are files so they never sit in the environment.

## The render worker

`stream-render` turns a recording into the one MP4 the runner streams. There is no browser: it draws the
frames itself (Skia, with the `board-draw`, `stage-draw` and `quiz-draw` crates, ports of the web player's
drawing) and runs `ffmpeg` and `ffprobe` as child processes. It needs the fonts (a directory), `ffmpeg` with
libx264 and an AAC encoder, and an API at a base URL you give it.

```
loop:
  POST {api}/lms/stream_worker/render/claim        204: nothing waits; 200: the request, the recording and a render token
  read the recording through the API (the newest render token on every read): the manifest, the board
      log, the deck pages and the teacher's video and sound files, downloaded to the work directory
  every 15 s: POST {api}/lms/stream_worker/render/{id}/heartbeat with the progress; the answer brings a
      fresh render token; {"stop": true} ends it
  make the video in pieces (below), the soundtrack in parallel (ffmpeg: adelay + amix), then join them
      (stream copy, +faststart); ffprobe (H.264 + AAC, at most 1280x720, length within the site's
      tolerance); POST .../render/{id}/uploads for the presigned part addresses; PUT each part (read from
      disk, never held in memory); POST .../render/{id}/finish with the ETags
```

- **The picture:** 1280x720 at a constant 30 fps. A countdown card (the lesson's chosen pre-roll), then the
  lesson: the teacher's shared screen, a quiz or Quest game, or the board, with the brand strip and the
  camera tile. Frames are converted to YUV 4:2:0 (BT.709, limited range) by the worker and encoded by x264
  (veryfast, crf 23, capped at 2.5 Mbit/s, no B-frames, a keyframe every 60 frames) as the runner's checks
  require.
- **Pieces:** the video is cut on keyframes into pieces drawn and encoded at the same time, each with its
  own threads and its own x264, and joined without re-encoding. The countdown card is a piece of its own,
  drawn once a second. The count is one piece per 10 s of lesson, up to the cores the worker may use;
  `RENDER_CHUNKS` fixes it. Two workers on a 4-vCPU host: `RENDER_CHUNKS=2` (measured: about 0.8 GiB each,
  and the quickest pair); one worker alone is quickest at the default.
- Only the recording's own addresses are read: the API's redirect to storage is followed by the worker
  (no token to storage), the board log's presigned address is read with no token, and nothing in an
  error message names an address.
- A render ends early, and its children are killed, when a heartbeat says stop or the lease is lost (no
  heartbeat gets through for 45 s), or it runs past twice the recording's length plus ten minutes. When
  the site said stop, answered 404 or 409, or was not reached for the whole lease, nothing is reported
  (the site knows); a local failure is reported once, as `render_failed`, or `recording_unreadable` when
  the recording cannot be read; nothing else is ever sent. A shutdown (SIGTERM) kills the children and
  removes the files without reporting: the site queues the render again when its lease runs out. The
  render's directory is removed in every case, and the work directory is emptied at start.
- Every log line goes through the redactor: render tokens, presigned addresses' query strings, the
  worker's token.

| Variable | Default | |
|---|---|---|
| `RENDER_API_BASE_URL` | required | API root, for example `https://example.org/api/v4` |
| `RENDER_TOKEN_FILE` | `/secrets/render-token` | the bearer token |
| `RENDER_WORK_DIR` | `/work` | where each render keeps its files; the worker owns everything in it |
| `RENDER_FONT_DIR` | `/fonts` | the board's faces and the fallback faces, in one directory (`crates/board-draw/README.md`) |
| `RENDER_FFMPEG`, `RENDER_FFPROBE` | `ffmpeg`, `ffprobe` | paths |
| `RENDER_CHUNKS` | by cores and length | how many pieces of the video are made at once (1 to 32) |
| `RENDER_PREFETCH_BYTES` | `1073741824` | the most of the recording's video files held on disk ahead of the frame being drawn |
| `RENDER_POLL_INTERVAL_SECS` | `5` | wait between claims when nothing waits |
| `RENDER_WORKER_ID` | `render-worker` | names this worker to the API (letters, digits, `-_.`, at most 64) |
| `RUST_LOG` | `info` | log filter |

Measured per render on a 4-vCPU host (OVH d2-8): a 10-minute board with a 60 s card takes 55 s and about
190 CPU-seconds alone, 103 s per pair with two at once; peak memory 0.8 GiB with `RENDER_CHUNKS=2`, 1.2 GiB
at four pieces. Size a unit for the cores it renders on and about 1 GiB per render (`render-compare` measures it:
`crates/render-compare`).

## Images and releases

Two images are published, both multi-architecture (`linux/amd64`, `linux/arm64`), both from the one tag:

| Image | Dockerfile | Holds |
|---|---|---|
| `ghcr.io/<owner>/stream-runner` | `Dockerfile` | the runner and ffmpeg (Debian 13) |
| `ghcr.io/<owner>/stream-render` | `render.Dockerfile` | the render worker, the fonts and ffmpeg (Ubuntu 24.04); no browser |

The web player's code is the git submodule `lesson-replay/` ([iamleson98/lesson-replay](https://github.com/iamleson98/lesson-replay),
which has a submodule of its own, `vendor/sui`): the render image takes the board's fonts, the countdown card's
strings and the logo from it, and `board-draw`, `stage-draw` and `quiz-draw` are ports of its drawing. Check out with `git submodule update --init`.
**Change the page in its own repository, then move the pointer here** (`git -C lesson-replay checkout <commit>`
and commit the pointer); dependabot opens that pull request weekly. Never edit files under `lesson-replay/` in this repository.

A release is a tag `vMAJOR.MINOR.PATCH` on a commit of `main` (`.github/workflows/image.yml`; there is no manual run):

1. `ancestry`: the tag's commit is on `main`, and the lesson-replay commit it pins is on lesson-replay's `main` (and
   `.gitmodules` points at the owner's repository), so a tag cannot ship a fork's commit.
2. `gate`: `ci.yml` (format, lints, tests, audit and the render image build).
3. `publish`: builds both images from scratch (no build cache), pushes the tags `vX.Y.Z` and `sha-<short>`, attaches an SBOM
   and build provenance, signs the index and each platform image with keyless cosign, and verifies them as a consumer would.
4. `release`: creates the GitHub Release. Its body has one line per image, in this shape (a script picks its image by it):

   ```
   image: ghcr.io/<owner>/stream-runner@sha256:<digest>
   image: ghcr.io/<owner>/stream-render@sha256:<digest>
   ```

   A published tag is never rebuilt: re-running a release that names other digests fails; cut a new version.

Verify an image (the identity is the workflow's file at the tag):

```bash
cosign verify ghcr.io/<owner>/stream-runner@sha256:<digest> \
  --certificate-identity https://github.com/<owner>/streamer-rs/.github/workflows/image.yml@refs/tags/vX.Y.Z \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
# the same for ghcr.io/<owner>/stream-render@sha256:<digest>
```

Build them locally (Docker with BuildKit; the render context is cut down by `render.Dockerfile.dockerignore`, the
runner's by `.dockerignore`, which leaves the submodule out):

```bash
docker build -t stream-runner .
git submodule update --init
docker build -f render.Dockerfile -t stream-render .
```

## Build and test

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked          # no network, no real ffmpeg
cargo audit --deny warnings && cargo deny check
cargo build --release --locked -p stream-runner -p stream-render
```

The runner has an off-by-default cargo feature, `test-ingest`, that lets `STREAM_TEST_INGEST_URL`
(an `rtmp://127.0.0.1` or `rtmp://localhost` address) replace the platform's fixed ingest address, so an
end-to-end test can send to a listener on the same machine. The feature also lets the file's address be
plain `http://` when its host is `localhost` or `127.0.0.1` (and named in `STREAM_MEDIA_HOST`), for a
store on the same machine. Without the feature that code is not
compiled, the variable is ignored and `http://` is always refused; no image or release build enables it
(`cargo test -p stream-runner --features test-ingest` runs its tests).

Some tests are ignored by default because they need real tools: `RENDER_TEST_FFMPEG=ffmpeg RENDER_TEST_FFPROBE=ffprobe cargo test -p stream-render --test real_media -- --ignored`
(a real `ffprobe` reading an MP4 made by a real `ffmpeg` with libx264, libopus and AAC; the paths absolute where the shell's `PATH` is not inherited) and
`RENDER_TEST_FFMPEG=ffmpeg RENDER_TEST_FFPROBE=ffprobe RENDER_TEST_FONTS=target/render-fonts cargo test -p stream-render --test native_media -- --ignored`
(the whole engine against the fake site: a board with a card, decks, a camera, a screen share, a quiz and
a Quest, and a video in four pieces with every frame once and a keyframe every 2 s; the fonts directory is
made as `crates/board-draw/README.md` says).

The toolchain is pinned in `rust-toolchain.toml`; dependencies are pinned to exact versions and
`Cargo.lock` is committed. The runner's tests run the whole loop against a fake API and fake `ffmpeg` and
`ffprobe` shell scripts (success, stop by heartbeat, failure then resume, a file the probe refuses,
a refused key, a lost lease, shutdown); the worker's against a fake API, fake storage and an engine double, and check that the key and the presigned signature appear in no
log line and no request body. They need a POSIX `sh` and nothing else.

What the tests do not cover: a real ffmpeg copying to a real ingest.

## Licence

MIT or Apache-2.0, at your option.
