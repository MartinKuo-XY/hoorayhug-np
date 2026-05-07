use std::io::Result;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::future::Future;

use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};

use super::{AsyncIOBuf, CopyBuffer};
use super::mem_copy::get_obfs_mode;

enum TransferState<B, SR, SW> {
    Running(CopyBuffer<B, SR, SW>),
    ShuttingDown(u64),
    Done(u64),
}

fn transfer<B, SL, SR>(
    cx: &mut Context<'_>,
    state: &mut TransferState<B, SL, SR>,
    r: &mut SL,
    w: &mut SR,
) -> Poll<Result<u64>>
where
    B: Unpin,
    SL: AsyncRead + AsyncWrite + Unpin,
    SR: AsyncRead + AsyncWrite + Unpin,
    CopyBuffer<B, SL, SR>: AsyncIOBuf<StreamR = SL, StreamW = SR>,
    CopyBuffer<B, SR, SL>: AsyncIOBuf<StreamR = SR, StreamW = SL>,
{
    loop {
        match state {
            TransferState::Running(buf) => {
                let count = ready!(buf.poll_copy(cx, r, w))?;
                *state = TransferState::ShuttingDown(count);
            }
            TransferState::ShuttingDown(count) => {
                ready!(Pin::new(&mut *w).poll_shutdown(cx))?;
                *state = TransferState::Done(*count);
            }
            TransferState::Done(count) => return Poll::Ready(Ok(*count)),
        }
    }
}

fn transfer2<B, SL, SR>(
    cx: &mut Context<'_>,
    state: &mut TransferState<B, SR, SL>,
    r: &mut SR,
    w: &mut SL,
) -> Poll<Result<u64>>
where
    B: Unpin,
    SL: AsyncRead + AsyncWrite + Unpin,
    SR: AsyncRead + AsyncWrite + Unpin,
    CopyBuffer<B, SL, SR>: AsyncIOBuf<StreamR = SL, StreamW = SR>,
    CopyBuffer<B, SR, SL>: AsyncIOBuf<StreamR = SR, StreamW = SL>,
{
    loop {
        match state {
            TransferState::Running(buf) => {
                let count = ready!(buf.poll_copy(cx, r, w))?;
                *state = TransferState::ShuttingDown(count);
            }
            TransferState::ShuttingDown(count) => {
                ready!(Pin::new(&mut *w).poll_shutdown(cx))?;
                *state = TransferState::Done(*count);
            }
            TransferState::Done(count) => return Poll::Ready(Ok(*count)),
        }
    }
}

struct BidiCopy<'a, B, SL, SR>
where
    B: Unpin,
    SL: AsyncRead + AsyncWrite + Unpin,
    SR: AsyncRead + AsyncWrite + Unpin,
    CopyBuffer<B, SL, SR>: AsyncIOBuf<StreamR = SL, StreamW = SR>,
    CopyBuffer<B, SR, SL>: AsyncIOBuf<StreamR = SR, StreamW = SL>,
{
    a: &'a mut SL,
    b: &'a mut SR,
    a_to_b: TransferState<B, SL, SR>,
    b_to_a: TransferState<B, SR, SL>,
}

impl<'a, B, SL, SR> Future for BidiCopy<'a, B, SL, SR>
where
    B: Unpin,
    SL: AsyncRead + AsyncWrite + Unpin,
    SR: AsyncRead + AsyncWrite + Unpin,
    CopyBuffer<B, SL, SR>: AsyncIOBuf<StreamR = SL, StreamW = SR>,
    CopyBuffer<B, SR, SL>: AsyncIOBuf<StreamR = SR, StreamW = SL>,
{
    type Output = Result<(u64, u64)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let BidiCopy { a, b, a_to_b, b_to_a } = self.get_mut();

        let a_to_b = transfer(cx, a_to_b, a, b)?;
        let b_to_a = transfer2::<B, SL, SR>(cx, b_to_a, b, a)?;

        #[cfg(not(feature = "brutal-shutdown"))]
        {
            let a_to_b = ready!(a_to_b);
            let b_to_a = ready!(b_to_a);
            Poll::Ready(Ok((a_to_b, b_to_a)))
        }

        #[cfg(feature = "brutal-shutdown")]
        {
            match (a_to_b, b_to_a) {
                (Poll::Ready(a), Poll::Ready(b)) => Poll::Ready(Ok((a, b))),
                (Poll::Pending, Poll::Ready(b)) => Poll::Ready(Ok((0, b))),
                (Poll::Ready(a), Poll::Pending) => Poll::Ready(Ok((a, 0))),
                _ => Poll::Pending,
            }
        }
    }
}

struct SimpleRng { state: u64 }
impl SimpleRng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        Self { state: seed }
    }
    fn next_range(&mut self, min: u32, max: u32) -> u32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let val = (self.state >> 32) as u32;
        min + (val % (max - min + 1))
    }
}

fn generate_dynamic_obfs(key: &str) -> Vec<u8> {
    let mut rng = SimpleRng::new();
    let num_digits = if rng.next_range(0, 1) == 0 { 2 } else { 3 };
    let mut digits = Vec::new();

    if num_digits == 2 {
        digits.push(rng.next_range(1, 9) as u8 + b'0');
        digits.push(rng.next_range(0, 9) as u8 + b'0');
    } else {
        let first = rng.next_range(0, 9) as u8;
        digits.push(first + b'0');
        if first == 0 {
            digits.push(rng.next_range(1, 9) as u8 + b'0');
            digits.push(rng.next_range(0, 9) as u8 + b'0');
        } else {
            digits.push(rng.next_range(0, 9) as u8 + b'0');
            digits.push(rng.next_range(0, 9) as u8 + b'0');
        }
    }

    let mut indices = vec![0, 1, 2, 3, 4];
    for i in 0..4 {
        let swap_idx = rng.next_range(i as u32, 4) as usize;
        indices.swap(i, swap_idx);
    }
    let mut digit_indices = indices[0..num_digits].to_vec();
    digit_indices.sort_unstable(); 

    let mut header = vec![0u8; 5];
    let mut digit_iter = digits.into_iter();

    for i in 0..5 {
        if digit_indices.contains(&i) {
            header[i] = digit_iter.next().unwrap();
        } else {
            let val = rng.next_range(0, 245) as u8; 
            header[i] = if val < 48 { val } else { val + 10 }; 
        }
    }

    let mut l_str = String::new();
    for &b in &header {
        if b.is_ascii_digit() { l_str.push(b as char); }
    }
    let garbage_len: usize = l_str.parse().unwrap();

    let key_bytes = key.as_bytes();
    if !key_bytes.is_empty() {
        for (i, byte) in header.iter_mut().enumerate() {
            *byte ^= key_bytes[i % key_bytes.len()];
        }
    }

    let mut garbage = vec![0u8; garbage_len];
    for b in garbage.iter_mut() {
        *b = rng.next_range(0, 255) as u8;
    }

    header.extend(garbage);
    header
}

async fn consume_dynamic_obfs<S>(stream: &mut S, key: &str) -> std::io::Result<()> 
where 
    S: tokio::io::AsyncRead + Unpin 
{
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).await?;

    let key_bytes = key.as_bytes();
    if !key_bytes.is_empty() {
        for (i, byte) in header.iter_mut().enumerate() {
            *byte ^= key_bytes[i % key_bytes.len()];
        }
    }

    let mut digits = Vec::new();
    for &b in &header {
        if b.is_ascii_digit() {
            digits.push(b);
        }
    }

    let mut is_probe = false;
    let len = digits.len();
    if len != 2 && len != 3 {
        is_probe = true;
    } else if len == 2 && digits[0] == b'0' {
        is_probe = true;
    } else if len == 3 && digits[0] == b'0' && digits[1] == b'0' {
        is_probe = true;
    }

    if is_probe {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Active probe silently dropped"));
    }

    let length_str = String::from_utf8(digits).unwrap();
    let l: usize = length_str.parse().unwrap();
    if l > 999 { 
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Length anomaly"));
    }

    let mut garbage = vec![0u8; l];
    stream.read_exact(&mut garbage).await?;
    Ok(())
}

pub async fn bidi_copy_buf<B, SR, SW>(
    a: &mut SR,
    b: &mut SW,
    a_to_b_buf: CopyBuffer<B, SR, SW>,
    b_to_a_buf: CopyBuffer<B, SW, SR>,
) -> Result<(u64, u64)>
where
    B: Unpin,
    SR: AsyncRead + AsyncWrite + Unpin,
    SW: AsyncRead + AsyncWrite + Unpin,
    CopyBuffer<B, SR, SW>: AsyncIOBuf<StreamR = SR, StreamW = SW>,
    CopyBuffer<B, SW, SR>: AsyncIOBuf<StreamR = SW, StreamW = SR>,
{
    let obfs_key = "hoorayhug_obfs_key_2024";

    match get_obfs_mode() {
        1 => { // Client 发送前缀
            let prefix = generate_dynamic_obfs(obfs_key);
            b.write_all(&prefix).await?;
            // ======================================================================
            // 【极其关键的修正】：彻底移除了 b.flush().await?;
            // 确保混淆头留在内核缓冲区，与接下来的第一次 Chunking 真实数据物理合并发车！
            // ======================================================================
        }
        2 => { // Server 解析并抵御探测
            if let Err(e) = consume_dynamic_obfs(a, obfs_key).await {
                return Err(e);
            }
        }
        _ => {} 
    }

    let a_to_b = TransferState::Running(a_to_b_buf);
    let b_to_a = TransferState::Running(b_to_a_buf);

    BidiCopy { a, b, a_to_b, b_to_a }.await
}
