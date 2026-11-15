## What happens

`docs/TNC.md` (Data Port) says the data port "only carries payload when a session is CONNECTED". With no session up, though, bytes written to the data port are accepted and queued (`BUFFER` reports them). They then go out as the first payload of the **next** session: with whichever station it is, outgoing or incoming, and after the data connection that wrote them has closed.

Found on 1.9.24 (274bd5c9, still the head of `mercuryv2`). A lab readiness check opened the data port, wrote one newline and closed. Mercury queued that byte. The next session, with a different client attached by then, sent `car_tx bytes=1` ahead of the host's first 231 bytes. The peer's application rejected the stream as malformed and closed the session.

A controlled check on one idle station (no session, no `CONNECT` pending):

| Step | Result |
|---|---|
| `BUFFER` | `BUFFER 0` |
| Write 30 bytes to the data port | `BUFFER 30` |
| `ABORT` | `OK`, `DISCONNECTED`, then `BUFFER 0` |

So `ABORT` clears them, but nothing else does until a session has carried them.

## The code (274bd5c9)

- `data_interfaces/tcp_interfaces.c`, data-port `POLLIN` (~L1337): every `recv` goes to `arq_submit_tcp_payload()`, whatever the session state.
- `datalink_arq/arq.c`: `g_app_tx_buf` is cleared at a session's end (~L377, "Flush stale TX bytes from the previous session"), on a `CONNECT` during a teardown (~L656) and by `ABORT` (`clear_connection_data`). Nothing clears it between sessions, or when the data client goes.
- The receive side already has the rule: on a data-client connect with no session up, `arq_discard_stale_rx()` drops bytes "some earlier session left behind: they were for a reader that is gone" (~L1310).

## Why it matters

Bytes meant for no one, or for an earlier peer, are delivered to whoever connects next, including an incoming caller. For a host that frames its stream (B2F, a chat app's own framing), one stray byte at the start breaks the session. A program that only probes the port is enough to cause it.

## Options

1. **Discard data written while there is no session and no outgoing call in progress**, with one warning in the log. Bytes written after `CONNECT`, while calling, would still be kept, since a host may start writing as soon as it sends `CONNECT`. Open question: does any VARA host write before `CONNECTED` and rely on it being kept? I don't know what VARA does here.
2. **Clear the queue when the data client disconnects with no session up**, the TX counterpart of `arq_discard_stale_rx()`. That covers the probe case and changes nothing for a host that stays connected.
3. **Keep the behaviour and document it:** bytes written while idle wait for the next session, and a host should send `ABORT` (or `CLEANTXBUFFER` once #561 is in) before it relies on the queue being empty.

Our host works around it for now: on attach it queries `BUFFER` and sends `ABORT` if anything is queued. I'm happy to send a PR for whichever option you prefer.
