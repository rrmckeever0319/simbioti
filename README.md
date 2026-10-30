# photosync

Encrypted photo and video sync between two Mac processes. The same Rust core builds as a static library SwiftUI can call on iOS.

v0 exchanges missing `jpg`, `jpeg`, `png`, `heic`, `mp4`, and `mov` files. A second run with the same folders transfers nothing.

## Layout

| Crate | Role |
| --- | --- |
| `core` (`photosync-core`) | Scan, BLAKE3 index, Noise session, sync state machine. Standard library only. No watcher and no async runtime. |
| `cli` (`photosync`) | macOS hub/client binary, logging, and folder watching. |
| `ffi` (`photosync-ffi`) | `staticlib` plus [`ffi/include/photosync.h`](ffi/include/photosync.h). |

Platform code stays in `cli` and `ffi`. iOS does not link the `notify` watcher.

## Keys and handshake

Both peers share one 32-byte key. There is no certificate authority.

```
photosync keygen
```

Put the hex in both configs as `psk_hex`. The handshake is Noise `NNpsk0_25519_ChaChaPoly_BLAKE2s` with prologue `photosync-v1`:

1. The client sends its ephemeral key. The pre-shared key authenticates that message.
2. The hub sends its ephemeral key and finishes the Diffie-Hellman exchange.
3. Both sides move into transport mode. Every later frame is ChaChaPoly ciphertext: a 4-byte big-endian length, then the Noise message.

A wrong key fails the handshake and the socket closes. NNpsk0 proves the peer knows the key. It does not name a device. That is the right scope for a shared home key. A future static-key pattern can use a new prologue without changing the frame layout.

## What is transferred

Files are identified by a streaming BLAKE3 hash. Each side sends a `have` list (one file per message, because a Noise frame cannot hold a huge manifest). Each side then pushes only hashes the other does not already have. File bytes travel in 48 KiB chunks, not as base64. The receiver checks the size and hash, then renames a temporary file into place.

Paths from the peer must be relative. `..` and absolute paths are rejected. Dotfiles, the index, and non-media files are ignored. If the destination name exists with a different hash, the new file is stored as `name.photosync-<hash8>.ext`.

The index is `{folder}/.photosync-index.json`. Unchanged size and mtime skip a rehash.

## Mac to Mac

`hub.toml`:

```toml
role = "hub"
folder = "/path/to/hub-photos"
listen = "127.0.0.1:8787"
psk_hex = "<64 hex chars>"
```

`client.toml`:

```toml
role = "client"
folder = "/path/to/client-photos"
peer = "127.0.0.1:8787"
psk_hex = "<64 hex chars>"
```

```
cargo run -p photosync -- hub --config hub.toml
cargo run -p photosync -- client --config client.toml
cargo run -p photosync -- client --config client.toml --watch
```

`listen = "127.0.0.1:0"` asks the OS for a port. The hub prints `PHOTOSYNC_LISTEN 127.0.0.1:<port>`. `--once` exits after one session. `--watch` syncs again about a second after a media file in the folder changes. Logs go to stderr. `RUST_LOG=debug` raises the level.

`scripts/demo.sh` runs a hub and a client in temporary folders.

## iOS static library

The C API is blocking. Call it from a Swift background task, not the main actor.

```swift
import photosync

var err = [CChar](repeating: 0, count: 512)
let cfg = photosync_config_new(folder, hubAddress, pskHex, 0, nil)
let rc = photosync_sync_once(cfg, &err, err.count)
photosync_config_free(cfg)
```

On iOS the app copies media into its sandbox folder, then calls `photosync_scan` and `photosync_sync_once`. v0 does not talk to PhotoKit. The phone is the client. The Mac runs `photosync hub`.

```
cargo build -p photosync-ffi --release
# target/release/libphotosync_ffi.a and ffi/include/photosync.h

rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build -p photosync-ffi --release --target aarch64-apple-ios
```

Add the static library to the Xcode target, add the header as a bridging header, and link `Security` only if the linker asks for it. The crate has no system TLS dependency.

## Tests

```
cargo test
```

That covers the handshake, a wrong key, index reuse, path checks, an in-process session over TCP, the C `scan` entry point, and two real `photosync` processes.
