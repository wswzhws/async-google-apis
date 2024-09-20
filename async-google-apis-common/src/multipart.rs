//! Format and stream a multipart/related request, for uploads.

use hyper::{self, body::Bytes};
use radix64::STD;
use serde::Serialize;
use std::io::Write;

use anyhow::Context;

pub const MIME_BOUNDARY: &str = "PB0BHe6XN3O6Q4bpnWQgS1pKfMfglTZdifFvh8YIc2APj4Cz3C";

pub fn format_multipart<Req: Serialize + std::fmt::Debug>(
    req: &Req,
    data: Bytes,
) -> anyhow::Result<Bytes> {
    let meta = serde_json::to_string(req).context(format!("{:?}", req))?;
    let mut buf = Vec::with_capacity(meta.len() + (1.5 * (data.len() as f64)) as usize);

    // Write metadata.
    buf.write_all(format!("--{}\n", MIME_BOUNDARY).as_bytes())
        .unwrap();
    buf.write_all("Content-Type: application/json; charset=UTF-8\n\n".as_bytes())
        .unwrap();
    buf.write_all(meta.as_bytes())?;

    buf.write_all(format!("\n\n--{}\n", MIME_BOUNDARY).as_bytes())
        .unwrap();
    buf.write_all("Content-Transfer-Encoding: base64\n\n".as_bytes())
        .unwrap();

    // write_all data contents.
    let mut ew = radix64::io::EncodeWriter::new(STD, buf);
    ew.write_all(data.as_ref())?;

    let mut buf = ew.finish()?;
    buf.write_all(format!("\n\n--{}--\n", MIME_BOUNDARY).as_bytes())
        .unwrap();
    Ok(Bytes::from(buf))
}
