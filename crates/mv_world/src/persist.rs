//! On-disk chunk format (PROMPT.md §5.2, ADR 011).
//!
//! ```text
//! magic     4 bytes  "CXCH"
//! version   u16 LE   CHUNK_VERSION
//! palette   u16 LE count, then per entry: u8 length + UTF-8 material name
//! voxels    runs of (palette index, run length), both LEB128 varints, covering
//!           the 512 bricks × 512 voxels in storage order (brick, then voxel)
//! checksum  u32 LE   Adler-32 of everything before it
//! ```
//!
//! Materials are stored by name, so a save survives materials being added or
//! reordered; unknown names load as air. Uniform bricks are single runs, so typical
//! modified terrain chunks take a few hundred bytes to a few KiB.

use mv_core::consts::{BRICK_VOLUME, BRICKS_PER_CHUNK};

use crate::{Brick, BrickRef, Chunk, MaterialId};

pub const CHUNK_MAGIC: [u8; 4] = *b"CXCH";
pub const CHUNK_VERSION: u16 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PersistError {
    #[error("not a chunk file (bad magic)")]
    BadMagic,
    #[error("chunk format version {0} is newer than this game supports ({CHUNK_VERSION})")]
    UnsupportedVersion(u16),
    #[error("chunk file is truncated")]
    Truncated,
    #[error("chunk file is corrupt (checksum mismatch)")]
    Corrupt,
    #[error("chunk file is malformed: {0}")]
    Malformed(&'static str),
}

/// A decoded chunk and how many voxels had a material this game does not know.
#[derive(Debug)]
pub struct Decoded {
    pub chunk: Chunk,
    pub unknown_voxels: usize,
}

/// Encodes `chunk`, naming its materials with `name`.
pub fn encode_chunk<'n>(chunk: &Chunk, name: impl Fn(MaterialId) -> &'n str) -> Vec<u8> {
    // Palette in order of first appearance.
    let mut palette: Vec<MaterialId> = Vec::new();
    let index_of = |m: MaterialId, palette: &mut Vec<MaterialId>| -> u64 {
        match palette.iter().position(|&p| p == m) {
            Some(i) => i as u64,
            None => {
                palette.push(m);
                (palette.len() - 1) as u64
            }
        }
    };
    let mut runs: Vec<u8> = Vec::new();
    let mut current: Option<(u64, u64)> = None;
    let push =
        |idx: u64, len: u64, runs: &mut Vec<u8>, current: &mut Option<(u64, u64)>| match current {
            Some((i, n)) if *i == idx => *n += len,
            _ => {
                if let Some((i, n)) = current.take() {
                    write_varint(runs, i);
                    write_varint(runs, n);
                }
                *current = Some((idx, len));
            }
        };
    for b in 0..BRICKS_PER_CHUNK {
        match chunk.brick(b) {
            BrickRef::Uniform(m) => {
                let idx = index_of(m, &mut palette);
                push(idx, BRICK_VOLUME as u64, &mut runs, &mut current);
            }
            BrickRef::Dense(voxels) => {
                for &m in voxels.iter() {
                    let idx = index_of(m, &mut palette);
                    push(idx, 1, &mut runs, &mut current);
                }
            }
        }
    }
    if let Some((i, n)) = current {
        write_varint(&mut runs, i);
        write_varint(&mut runs, n);
    }

    let mut out = Vec::with_capacity(16 + palette.len() * 8 + runs.len());
    out.extend_from_slice(&CHUNK_MAGIC);
    out.extend_from_slice(&CHUNK_VERSION.to_le_bytes());
    out.extend_from_slice(&(palette.len() as u16).to_le_bytes());
    for &m in &palette {
        let n = name(m).as_bytes();
        let n = &n[..n.len().min(255)];
        out.push(n.len() as u8);
        out.extend_from_slice(n);
    }
    out.extend_from_slice(&runs);
    let sum = adler32(&out);
    out.extend_from_slice(&sum.to_le_bytes());
    out
}

/// Decodes a chunk file, resolving material names with `id` (unknown names become
/// air). The result is marked modified (it differs from the generator).
pub fn decode_chunk(
    bytes: &[u8],
    id: impl Fn(&str) -> Option<MaterialId>,
) -> Result<Decoded, PersistError> {
    if bytes.len() < 4 + 2 + 2 + 4 {
        return Err(PersistError::Truncated);
    }
    if bytes[..4] != CHUNK_MAGIC {
        return Err(PersistError::BadMagic);
    }
    let (body, sum) = bytes.split_at(bytes.len() - 4);
    let version = u16::from_le_bytes([body[4], body[5]]);
    if version > CHUNK_VERSION {
        return Err(PersistError::UnsupportedVersion(version));
    }
    if adler32(body) != u32::from_le_bytes([sum[0], sum[1], sum[2], sum[3]]) {
        return Err(PersistError::Corrupt);
    }
    let mut r = Reader {
        bytes: body,
        pos: 6,
    };
    let count = r.u16()? as usize;
    let mut palette = Vec::with_capacity(count);
    let mut unknown = vec![false; count];
    for slot in unknown.iter_mut() {
        let len = r.u8()? as usize;
        let name = std::str::from_utf8(r.take(len)?)
            .map_err(|_| PersistError::Malformed("material name is not UTF-8"))?;
        let m = id(name);
        *slot = m.is_none();
        palette.push(m.unwrap_or(MaterialId::AIR));
    }

    const TOTAL: usize = BRICKS_PER_CHUNK * BRICK_VOLUME;
    let mut scratch = [MaterialId::AIR; BRICK_VOLUME];
    let mut bricks: Vec<Brick> = Vec::with_capacity(BRICKS_PER_CHUNK);
    let (mut filled, mut unknown_voxels) = (0usize, 0usize);
    while filled < TOTAL {
        let idx = r.varint()? as usize;
        let len = r.varint()? as usize;
        let m = *palette
            .get(idx)
            .ok_or(PersistError::Malformed("palette index out of range"))?;
        if len == 0 || filled + len > TOTAL {
            return Err(PersistError::Malformed("bad run length"));
        }
        if unknown[idx] {
            unknown_voxels += len;
        }
        let mut left = len;
        while left > 0 {
            let at = filled % BRICK_VOLUME;
            let n = left.min(BRICK_VOLUME - at);
            scratch[at..at + n].fill(m);
            filled += n;
            left -= n;
            if filled % BRICK_VOLUME == 0 {
                bricks.push(Brick::from_voxels(&scratch));
            }
        }
    }
    if r.pos != body.len() {
        return Err(PersistError::Malformed("trailing data"));
    }
    let table: Box<crate::BrickTable> = bricks
        .into_boxed_slice()
        .try_into()
        .map_err(|_| PersistError::Malformed("wrong brick count"))?;
    let mut chunk = Chunk::from_bricks(table);
    chunk.mark_modified();
    Ok(Decoded {
        chunk,
        unknown_voxels,
    })
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], PersistError> {
        let end = self.pos.checked_add(n).ok_or(PersistError::Truncated)?;
        let s = self
            .bytes
            .get(self.pos..end)
            .ok_or(PersistError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, PersistError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PersistError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn varint(&mut self) -> Result<u64, PersistError> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(PersistError::Malformed("varint too long"))
    }
}

/// Adler-32 (RFC 1950): cheap and enough to catch truncation or bit rot.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_core::{ChunkPos, LocalPos};

    const NAMES: [&str; 4] = ["air", "stone", "sand", "water"];

    fn name(m: MaterialId) -> &'static str {
        NAMES[m.index()]
    }

    fn id(n: &str) -> Option<MaterialId> {
        NAMES
            .iter()
            .position(|&x| x == n)
            .map(|i| MaterialId(i as u16))
    }

    fn same(a: &Chunk, b: &Chunk) -> bool {
        (0..64u8).all(|z| {
            (0..64u8).all(|y| {
                (0..64u8).all(|x| a.get(LocalPos::new(x, y, z)) == b.get(LocalPos::new(x, y, z)))
            })
        })
    }

    #[test]
    fn uniform_and_edited_chunks_round_trip_small() {
        let c = Chunk::uniform(MaterialId(1));
        let bytes = encode_chunk(&c, name);
        assert!(
            bytes.len() < 40,
            "a uniform chunk is tiny: {} bytes",
            bytes.len()
        );
        let d = decode_chunk(&bytes, id).unwrap();
        assert!(same(&c, &d.chunk));
        assert!(d.chunk.is_modified());

        let mut c = Chunk::from_fn(ChunkPos::new(0, 0, 0), |w| {
            if w.0.y < 20 {
                MaterialId(1)
            } else {
                MaterialId::AIR
            }
        });
        for i in 0..200u8 {
            c.set(
                LocalPos::new(i % 64, 20 + i / 64, (i * 7) % 64),
                MaterialId(2),
            );
        }
        let bytes = encode_chunk(&c, name);
        assert!(bytes.len() < 4096, "{} bytes", bytes.len());
        let d = decode_chunk(&bytes, id).unwrap();
        assert!(same(&c, &d.chunk));
        assert_eq!(d.unknown_voxels, 0);
    }

    #[test]
    fn noisy_chunks_round_trip() {
        let c = Chunk::from_fn(ChunkPos::new(-3, 1, 2), |w| {
            let h = (w.0.x * 31 + w.0.y * 17 + w.0.z * 7).rem_euclid(5) as u16;
            MaterialId(h.min(3))
        });
        let d = decode_chunk(&encode_chunk(&c, name), id).unwrap();
        assert!(same(&c, &d.chunk));
    }

    #[test]
    fn materials_are_stored_by_name() {
        let c = Chunk::uniform(MaterialId(3)); // "water"
        let bytes = encode_chunk(&c, name);
        // A game where "water" got a different id.
        let renumbered = |n: &str| match n {
            "water" => Some(MaterialId(9)),
            _ => None,
        };
        let d = decode_chunk(&bytes, renumbered).unwrap();
        assert_eq!(d.chunk.as_uniform(), Some(MaterialId(9)));
        // A game without it: air, reported.
        let d = decode_chunk(&bytes, |_| None).unwrap();
        assert_eq!(d.chunk.as_uniform(), Some(MaterialId::AIR));
        assert_eq!(d.unknown_voxels, 64 * 64 * 64);
    }

    #[test]
    fn damaged_files_are_rejected() {
        let bytes = encode_chunk(&Chunk::uniform(MaterialId(1)), name);
        let mut flipped = bytes.clone();
        flipped[8] ^= 0x10;
        assert_eq!(
            decode_chunk(&flipped, id).unwrap_err(),
            PersistError::Corrupt
        );
        assert_eq!(
            decode_chunk(&bytes[..bytes.len() - 6], id).unwrap_err(),
            PersistError::Corrupt
        );
        assert_eq!(
            decode_chunk(b"nope", id).unwrap_err(),
            PersistError::Truncated
        );
        let mut magic = bytes.clone();
        magic[0] = b'X';
        assert_eq!(
            decode_chunk(&magic, id).unwrap_err(),
            PersistError::BadMagic
        );
        let mut newer = bytes.clone();
        newer[4] = 99;
        assert_eq!(
            decode_chunk(&newer, id).unwrap_err(),
            PersistError::UnsupportedVersion(99)
        );
    }

    #[test]
    fn adler32_matches_reference() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }
}
