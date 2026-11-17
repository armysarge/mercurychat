//! Lab F5: Mercury serves one broadcast client, so a second may wait in the
//! OS backlog. Closing MercuryChat's socket must send RST. On Windows this
//! also discards queued bytes before Mercury can read them. On Linux, bytes
//! already in the receiver's queue can survive the reset.

use mercury_adapter::broadcast::BroadcastPort;
use std::io::ErrorKind;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

const T: Duration = Duration::from_secs(2);

/// What a late reader of `sock` gets: the bytes it read before the end, and
/// the error that ended it (`None` for a clean end of stream).
async fn late_read(mut sock: TcpStream) -> (usize, Option<ErrorKind>) {
    let mut got = 0;
    let mut buf = [0u8; 1024];
    loop {
        match tokio::time::timeout(T, sock.read(&mut buf)).await {
            Err(_) => panic!("the connection neither ended nor reset within {T:?}"),
            Ok(Ok(0)) => return (got, None),
            Ok(Ok(n)) => got += n,
            Ok(Err(e)) => return (got, Some(e.kind())),
        }
    }
}

fn assert_reset((received, error): (usize, Option<ErrorKind>)) {
    assert_eq!(error, Some(ErrorKind::ConnectionReset));
    if cfg!(windows) {
        assert_eq!(received, 0, "a frame survived the reset");
    }
}

/// Let the reset reach the other end.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn closing_a_backlogged_connection_resets_it() {
    // Mercury's view of a second client: the listener does not accept yet.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (bp, _rx) = BroadcastPort::connect("127.0.0.1", port, T).await.unwrap();
    bp.send(b"a stale beacon").await.unwrap();
    drop(bp);
    settle().await;
    // The holder lets go: Mercury accepts what is waiting, if anything is.
    match tokio::time::timeout(T, l.accept()).await {
        Err(_) => {} // the reset took it out of the backlog: nothing to read
        Ok(Err(e)) => {
            assert!(matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted), "accept: {e}")
        }
        Ok(Ok((sock, _))) => {
            assert_reset(late_read(sock).await);
        }
    }
}

#[tokio::test]
async fn aborting_through_a_handle_resets_while_the_port_is_still_held() {
    // The actor resets the connection the modem task still holds.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (bp, _rx) = BroadcastPort::connect("127.0.0.1", port, T).await.unwrap();
    let abort = bp.abort_handle();
    bp.send(b"a stale beacon").await.unwrap();
    abort.abort();
    abort.abort(); // idempotent
    settle().await;
    assert!(bp.is_closed(), "an aborted port says so");
    let refused = bp.send(b"a second beacon").await.unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::NotConnected, "nothing more is written on it");
    match tokio::time::timeout(T, l.accept()).await {
        Err(_) => {}
        Ok(Err(e)) => assert!(matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted)),
        Ok(Ok((sock, _))) => assert_reset(late_read(sock).await),
    }
}

#[tokio::test]
async fn closing_an_unread_connection_resets_it() {
    // Accepted but never read until later (a stalled reader).
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (bp, _rx) = BroadcastPort::connect("127.0.0.1", port, T).await.unwrap();
    let (sock, _) = l.accept().await.unwrap();
    bp.send(b"a stale beacon").await.unwrap();
    settle().await;
    drop(bp);
    settle().await;
    assert_reset(late_read(sock).await);
}
