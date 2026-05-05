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
    w.write_all(&len.to_le_bytes()).await.context("write length")?;
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
