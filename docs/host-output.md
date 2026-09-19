# Host attachment output

The imported host keeps the original JSON-line acknowledgement and byte-frame
wire format, including `guarded_v1`'s exact instance acknowledgement. Each
attachment now has one bounded FIFO and one socket writer. The PTY reader only
admits frames to these queues; it never writes to or waits for a client socket.
Disconnecting the Web bridge still only detaches its connection, leaving the
independent host and its child running.

## Reproduced failures and ordering

Two deterministic socket tests failed on the original implementation:

- Pausing attach after client registration and publishing live output before
  sending its ACK produced a byte frame before the JSON line.
- Filling a local socket before publishing showed `Client::send` blocking the
  PTY publisher until the peer disconnected.

The original model also fed a piece into the screen before clearing its
`inflight` entry under a different lock. A snapshot in between could include that
piece in both the rendered screen and its raw pending suffix.

The new attach boundary locks screen, then backlog. It takes the model snapshot
and unapplied suffix, atomically admits ACK plus replay, and registers the client
before releasing that boundary. PTY publication admits live frames while holding
backlog. Thus a piece belongs to either the replay or the subsequent live stream,
and socket output is always ACK → replay → live → exit. Replay is a terminal
state reconstruction, not a complete historical byte log. Existing screen
history limits and the screen backlog's reported `dropped` behavior remain.

The screen worker retains the screen lock until it clears `inflight` and advances
the applied count. Snapshots use the same screen → backlog lock order, preventing
an already applied piece from being replayed again as a pending suffix. Parsing
does not hold backlog; no socket write or thread join holds either model lock.

## Resource and disconnect policy

| Boundary | Limit | Result when exceeded |
| --- | --- | --- |
| Per-session attachment reservations | 32, including pending attaches | JSON `attach_limit` error before resize, registration or writer creation |
| Per-attachment outstanding output | 8 MiB and 1,024 queue entries | Disconnect that attachment; never drop arbitrary live bytes and continue |
| One frame's socket write | 2 seconds, absolute elapsed deadline | Disconnect that attachment, including a peer making tiny partial progress |
| PTY drain after the owned child exits | 3 seconds awaiting real EOF (600 ms on Windows, where ConPTY never delivers EOF to its owner and the elapsed drain is the regular exit) | Exit with explicit `output_complete: false` and `reason: "pty_drain_timeout"` (Unix) |
| All writers' final exit drain | One shared 2-second deadline | Close unfinished attachments together; never wait 2 seconds per peer |

Outstanding accounting includes the frame currently in the socket writer.
Broadcast frames share their immutable allocation. ACK and replay admission is
atomic: an initial replay too large for the queue disconnects before a success
ACK can be emitted. Replay construction itself still uses the configured screen
history and screen backlog before the output queue admits it.

Overflow and write errors log a generic local reason, and socket shutdown wakes
both the blocked output writer and the attachment input reader. There is no
extra wire error frame: an already stalled stream could not reliably deliver
one, and adding a frame type would change compatibility. Reconnect may request a
fresh snapshot; the host does not retry input or replay an ambiguous command.

Child exit is not PTY EOF: the reader continues publishing delayed or buffered
bytes for up to 3 seconds after the host observes its owned child's exit. Real EOF
finishes normally. An indefinitely retained slave descriptor triggers explicit
incompleteness instead; the host does not enumerate or kill unknown descendants.
The imported 200 ms forced finish was reproduced discarding a delayed PTY tail
and has been replaced by this bounded drain. On Windows the drain is 600 ms
and its end is the regular exit (`output_complete: true`, no `reason`):
ConPTY keeps the output pipe open as long as the host owns the pseudo console,
so EOF never arrives, conhost has rendered the child's final writes within a
few frames of its exit, and a 3 s wait made every stop report `uncertain`
although the host was about to finish. Completeness on Windows
therefore means "the drain window elapsed after the child's exit", not EOF.

Exit payloads add `output_complete: true` on EOF. PTY drain timeout or a read
failure adds `output_complete: false` and `reason: "pty_drain_timeout"` or
`"pty_read_error"`; the original `code` and frame type remain. The portable-pty
Unix reader already normalizes terminal-close `EIO` to EOF. Completeness refers
to reading the PTY to EOF, not unlimited screen history or confirmation that a
remote browser rendered every byte. A socket disconnected for overflow or a
write deadline cannot reliably receive any exit marker.

Finishing prevents new registration/publication under backlog, queues exit after
existing data, and lets writers drain concurrently before marking the session
exited. The shared 2-second socket drain deadline is separate from the 3-second
PTY drain, the existing model synchronization wait, and the child-exit wait. No
reader/model lock is held while waiting for PTY EOF or socket drainage.

## Validation

`cargo test -p ptyhost --test host_output --locked` launches only a fixed free `/bin/sh` fixture
with an explicit private temporary `--dir`. It checks both legacy and guarded
attach, final data before exit and EOF, the child exit code, and capacity rejection
before resize while all existing connections remain usable. A test-owned slave
descriptor deterministically delays EOF after the owned child is reaped: a tail
written 600 ms later is retained, and holding it indefinitely produces the
3-second incomplete exit marker. No descendant process is needed. Cleanup owns
only that fixture's host, synthetic child, descriptors, and temporary directory.

Linux tests and `cargo check -p ptyhost --locked --target
x86_64-pc-windows-msvc` have passed. The cross-check compiles the TCP write-timeout
branch; it is not Windows or macOS runtime validation.

The client additionally checks typed exit metadata; the Web bridge keeps output
ahead of close and distinguishes an unmarked socket EOF from an explicit host
exit. `tests/terminal_exit_browser.py` verifies the real legacy xterm tail and
specific exit explanation on desktop/mobile, without automatic reclaims of an
exited instance. See [terminal ownership](terminal-ownership.md).
