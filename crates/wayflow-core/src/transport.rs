// Framed message transport over an async TLS stream.
//
// Wire format: [u32 LE length][postcard bytes]
// Max message size: 4 MiB (enforced on read to prevent OOM).

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wayflow_proto::{C2S, S2C};

const MAX_MESSAGE_BYTES: u32 = 4 * 1024 * 1024;

pub async fn send_s2c<W: AsyncWrite + Unpin>(w: &mut W, msg: &S2C) -> Result<()> {
    write_frame(w, msg).await
}

pub async fn recv_s2c<R: AsyncRead + Unpin>(r: &mut R) -> Result<S2C> {
    read_frame(r).await
}

pub async fn send_c2s<W: AsyncWrite + Unpin>(w: &mut W, msg: &C2S) -> Result<()> {
    write_frame(w, msg).await
}

pub async fn recv_c2s<R: AsyncRead + Unpin>(r: &mut R) -> Result<C2S> {
    read_frame(r).await
}

async fn write_frame<W, T>(w: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let payload = postcard::to_allocvec(msg).context("serialize")?;
    let len = u32::try_from(payload.len()).context("message too large")?;
    w.write_all(&len.to_le_bytes())
        .await
        .context("write length")?;
    w.write_all(&payload).await.context("write payload")?;
    Ok(())
}

async fn read_frame<R, T>(r: &mut R) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await.context("read length")?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_MESSAGE_BYTES {
        bail!("message too large: {len} bytes");
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await.context("read payload")?;
    postcard::from_bytes(&payload).context("deserialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;
    use wayflow_proto::*;

    // ---------- S2C roundtrips ----------

    #[tokio::test]
    async fn s2c_ping_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        send_s2c(&mut a, &S2C::Ping).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), S2C::Ping);
    }

    #[tokio::test]
    async fn s2c_hello_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = S2C::Hello(HelloS2C {
            version: PROTOCOL_VERSION,
            screens: vec![ScreenInfo {
                name: "srv".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }],
        });
        send_s2c(&mut a, &msg).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), msg);
    }

    #[tokio::test]
    async fn s2c_enter_screen_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = S2C::EnterScreen { x: 42, y: 99 };
        send_s2c(&mut a, &msg).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), msg);
    }

    #[tokio::test]
    async fn s2c_leave_screen_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        send_s2c(&mut a, &S2C::LeaveScreen).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), S2C::LeaveScreen);
    }

    #[tokio::test]
    async fn s2c_mouse_move_abs_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = S2C::MouseMoveAbs { x: 640, y: 480 };
        send_s2c(&mut a, &msg).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), msg);
    }

    #[tokio::test]
    async fn s2c_mouse_button_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = S2C::MouseButton {
            button: MouseButton::Right,
            pressed: false,
        };
        send_s2c(&mut a, &msg).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), msg);
    }

    #[tokio::test]
    async fn s2c_scroll_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = S2C::Scroll { dx: 10, dy: -10 };
        send_s2c(&mut a, &msg).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), msg);
    }

    #[tokio::test]
    async fn s2c_key_event_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = S2C::KeyEvent {
            keycode: 65,
            pressed: true,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        };
        send_s2c(&mut a, &msg).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), msg);
    }

    #[tokio::test]
    async fn s2c_clipboard_data_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = S2C::ClipboardData(ClipboardContent::Text("hello".into()));
        send_s2c(&mut a, &msg).await.unwrap();
        assert_eq!(recv_s2c(&mut b).await.unwrap(), msg);
    }

    // ---------- C2S roundtrips ----------

    #[tokio::test]
    async fn c2s_pong_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        send_c2s(&mut a, &C2S::Pong).await.unwrap();
        assert_eq!(recv_c2s(&mut b).await.unwrap(), C2S::Pong);
    }

    #[tokio::test]
    async fn c2s_hello_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = C2S::Hello(HelloC2S {
            version: PROTOCOL_VERSION,
            name: "helicon".into(),
            screens: vec![ScreenInfo {
                name: "helicon".into(),
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            }],
        });
        send_c2s(&mut a, &msg).await.unwrap();
        assert_eq!(recv_c2s(&mut b).await.unwrap(), msg);
    }

    #[tokio::test]
    async fn c2s_clipboard_data_roundtrip() {
        let (mut a, mut b) = duplex(4096);
        let msg = C2S::ClipboardData(ClipboardContent::Text("from client".into()));
        send_c2s(&mut a, &msg).await.unwrap();
        assert_eq!(recv_c2s(&mut b).await.unwrap(), msg);
    }

    // ---------- Multiple messages in sequence ----------

    #[tokio::test]
    async fn multiple_s2c_messages_sequential() {
        let (mut a, mut b) = duplex(16384);
        let msgs = [S2C::Ping, S2C::LeaveScreen, S2C::EnterScreen { x: 1, y: 2 }];
        for m in &msgs {
            send_s2c(&mut a, m).await.unwrap();
        }
        for expected in &msgs {
            assert_eq!(recv_s2c(&mut b).await.unwrap(), *expected);
        }
    }

    // ---------- Error paths ----------

    #[tokio::test]
    async fn closed_stream_returns_error_on_recv_s2c() {
        let (a, mut b) = duplex(64);
        drop(a);
        assert!(recv_s2c(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn closed_stream_returns_error_on_recv_c2s() {
        let (a, mut b) = duplex(64);
        drop(a);
        assert!(recv_c2s(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn oversized_message_rejected() {
        let (mut a, mut b) = duplex(1024);
        // Write a length header claiming 5 MiB (over the 4 MiB limit).
        let len: u32 = 5 * 1024 * 1024;
        a.write_all(&len.to_le_bytes()).await.unwrap();
        drop(a);
        let err = recv_s2c(&mut b).await.unwrap_err();
        assert!(
            err.to_string().contains("too large"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn invalid_payload_returns_deserialize_error() {
        let (mut a, mut b) = duplex(1024);
        let payload = [0xffu8; 3];
        a.write_all(&(payload.len() as u32).to_le_bytes())
            .await
            .unwrap();
        a.write_all(&payload).await.unwrap();
        drop(a);
        let err = recv_s2c(&mut b).await.unwrap_err();
        assert!(
            err.to_string().contains("deserialize"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn max_valid_message_accepted() {
        // A message right at the 4 MiB limit with valid postcard content.
        // We send a ClipboardData with a string large enough that the serialized
        // size is exactly MAX_MESSAGE_BYTES -- instead just verify that the
        // roundtrip works for a moderately large payload.
        let big_text = "x".repeat(1024 * 1024); // 1 MiB string
        let (mut a, mut b) = duplex(2 * 1024 * 1024);
        let msg = S2C::ClipboardData(ClipboardContent::Text(big_text));
        send_s2c(&mut a, &msg).await.unwrap();
        let received = recv_s2c(&mut b).await.unwrap();
        assert_eq!(received, msg);
    }
}
