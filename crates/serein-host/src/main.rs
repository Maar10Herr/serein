use serein_core::*;
use std::io::{Read, Write};
fn run() -> Result<serde_json::Value> {
    let mut input = std::io::stdin().lock();
    let mut head = [0u8; 4];
    input
        .read_exact(&mut head)
        .map_err(|_| invalid("Incomplete native frame."))?;
    let n = u32::from_ne_bytes(head) as usize;
    if n == 0 || n > 256 * 1024 {
        return Err(invalid("Native frame exceeds limit."));
    }
    let mut bytes = vec![0; n];
    input
        .read_exact(&mut bytes)
        .map_err(|_| invalid("Incomplete native frame."))?;
    let env: Envelope = serde_json::from_slice(&bytes)?;
    install::host_dispatch(&root(), env, &std::env::args().skip(1).collect::<Vec<_>>())
}
fn main() {
    let value = run().unwrap_or_else(|e| error_value(&e));
    let mut bytes = serde_json::to_vec(&value).unwrap();
    if bytes.len() > 256 * 1024 {
        bytes = serde_json::to_vec(&error_value(&invalid("Response exceeds limit."))).unwrap()
    }
    let mut out = std::io::stdout().lock();
    let _ = out
        .write_all(&(bytes.len() as u32).to_ne_bytes())
        .and_then(|_| out.write_all(&bytes))
        .and_then(|_| out.flush());
}
