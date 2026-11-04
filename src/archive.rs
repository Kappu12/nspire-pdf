// TI method 13 decoding follows the MIT-licensed TnsTools format research.
use anyhow::{Context, Result, bail, ensure};
use des::TdesEde3;
use des::cipher::{Block, BlockDecrypt, BlockEncrypt, KeyInit};
use flate2::read::DeflateDecoder;
use std::{collections::BTreeMap, io::Read};

pub const LIMIT: usize = 64 * 1024 * 1024;

pub struct Archive<'a>(pub BTreeMap<&'a str, Entry<'a>>);

pub struct Entry<'a> {
    method: u16,
    crc: u32,
    size: usize,
    payload: &'a [u8],
}

fn bytes(data: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    data.get(start..)
        .and_then(|tail| tail.get(..len))
        .context("truncated archive record")
}

fn u16_at(data: &[u8], start: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(data, start, 2)?.try_into()?))
}

fn u32_at(data: &[u8], start: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(data, start, 4)?.try_into()?))
}

impl<'a> Archive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        ensure!(
            data.starts_with(b"*TIMLP") || data.starts_with(b"PK\x03\x04"),
            "expected a TI-Nspire TNS archive"
        );
        let end = (data.len().saturating_sub(65_557)..data.len().saturating_sub(21))
            .rev()
            .find(|&i| {
                matches!(&data[i..i + 4], b"TIPD" | b"PK\x05\x06")
                    && u16_at(data, i + 20).is_ok_and(|n| i + 22 + usize::from(n) == data.len())
            })
            .context("missing archive directory footer")?;
        ensure!(
            u32_at(data, end + 4)? == 0,
            "multi-disk archives are unsupported"
        );
        let count = u16_at(data, end + 10)?;
        ensure!(
            u16_at(data, end + 8)? == count,
            "inconsistent directory entry counts"
        );
        let start = u32_at(data, end + 16)? as usize;
        let directory = bytes(data, start, u32_at(data, end + 12)? as usize)?;
        ensure!(start + directory.len() == end, "invalid directory size");
        let mut cursor = 0;
        let mut entries = BTreeMap::new();

        for _ in 0..count {
            let header = bytes(directory, cursor, 46)?;
            ensure!(&header[..4] == b"PK\x01\x02", "invalid directory entry");
            ensure!(u16_at(header, 8)? & 1 == 0, "ZIP encryption is unsupported");
            let name_len = usize::from(u16_at(header, 28)?);
            let extra_len = usize::from(u16_at(header, 30)?);
            let comment_len = usize::from(u16_at(header, 32)?);
            let name = str::from_utf8(bytes(directory, cursor + 46, name_len)?)?;
            cursor += 46 + name_len + extra_len + comment_len;

            let local = u32_at(header, 42)? as usize;
            let prefix_len = match bytes(data, local, 4)? {
                b"*TIM" if local == 0 && data.starts_with(b"*TIMLP") => 10,
                b"PK\x03\x04" => 4,
                _ => bail!("invalid local header for {name}"),
            };
            let local_header = bytes(data, local + prefix_len, 26)?;
            let local_name_len = usize::from(u16_at(local_header, 22)?);
            let local_extra_len = usize::from(u16_at(local_header, 24)?);
            let local_name = local + prefix_len + 26;
            ensure!(
                bytes(data, local_name, local_name_len)? == name.as_bytes(),
                "mismatched local name for {name}"
            );
            let payload = bytes(
                &data[..start],
                local_name + local_name_len + local_extra_len,
                u32_at(header, 20)? as usize,
            )?;
            let entry = Entry {
                method: u16_at(header, 10)?,
                crc: u32_at(header, 16)?,
                size: u32_at(header, 24)? as usize,
                payload,
            };
            ensure!(
                entries.insert(name, entry).is_none(),
                "duplicate entry {name}"
            );
        }
        ensure!(
            cursor == directory.len(),
            "unexpected data in archive directory"
        );
        Ok(Self(entries))
    }
}

impl Entry<'_> {
    pub fn decode(&self) -> Result<Vec<u8>> {
        ensure!(self.size <= LIMIT, "archive entry exceeds the size limit");
        let result = match self.method {
            0 => self.payload.to_vec(),
            8 => inflate(self.payload, self.size)?,
            13 => {
                let inflated = method13(self.payload)?;
                if inflated.starts_with(b"TIXC0100") {
                    crate::tixc::expand(&inflated, self.size)?
                } else {
                    inflated
                }
            }
            method => bail!("unsupported compression method {method}"),
        };
        ensure!(result.len() == self.size, "incorrect decoded size");
        ensure!(crc32fast::hash(&result) == self.crc, "incorrect entry CRC");
        Ok(result)
    }
}

fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    DeflateDecoder::new(data)
        .take(limit as u64 + 1)
        .read_to_end(&mut output)?;
    ensure!(
        output.len() <= limit,
        "inflated data exceeds the size limit"
    );
    Ok(output)
}

fn method13(data: &[u8]) -> Result<Vec<u8>> {
    const HEADER_KEY: [u8; 24] = [
        0x79, 0xc4, 0xe0, 0xf4, 0x5e, 0xef, 0x7a, 0x5b, 0x70, 0x13, 0x7a, 0x57, 0xc2, 0xfd, 0x3d,
        0x2c, 0xc2, 0x70, 0x7c, 0xc1, 0xad, 0x2f, 0x15, 0x75,
    ];
    let mut header: [u8; 40] = bytes(data, 0, 40)?.try_into()?;
    let cipher = TdesEde3::new((&HEADER_KEY).into());
    for chunk in header.chunks_exact_mut(8) {
        cipher.decrypt_block(Block::<TdesEde3>::from_mut_slice(chunk));
    }
    ensure!(
        &header[..8] == b"TIEN0100",
        "unrecognized TI encryption header"
    );
    let period = u32_at(&header, 8)?;
    ensure!(period == 0x400, "unsupported TI counter period {period}");
    let seed = u32_at(&header, 12)?;

    let mut key = [0; 24];
    for (packed, expanded) in header[16..37].chunks_exact(7).zip(key.chunks_exact_mut(8)) {
        let mut bits = [0; 8];
        bits[1..].copy_from_slice(packed);
        let bits = u64::from_be_bytes(bits);
        for (i, byte) in expanded.iter_mut().enumerate() {
            *byte = ((bits >> (49 - 7 * i)) as u8 & 0x7f) << 1;
            *byte |= (byte.count_ones() % 2 == 0) as u8;
        }
    }

    let cipher = TdesEde3::new((&key).into());
    let mut body = data[40..].to_vec();
    for (i, chunk) in body.chunks_mut(8).enumerate() {
        let mut counter = Block::<TdesEde3>::default();
        counter[4..].copy_from_slice(&seed.wrapping_add(i as u32 % period).to_le_bytes());
        cipher.encrypt_block(&mut counter);
        for (byte, mask) in chunk.iter_mut().zip(counter) {
            *byte ^= mask;
        }
    }
    inflate(&body, LIMIT)
}
