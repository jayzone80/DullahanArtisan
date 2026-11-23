/// Pure-Rust zero-dependency Zlib (RFC 1950) + DEFLATE (RFC 1951) compressor and decompressor.
/// Uses LZ77 hash-chain matching with Fixed Huffman blocks for fast, compact PNG compression
/// and supports full uncompressed, fixed Huffman, and dynamic Huffman decoding for verification.

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

struct BitWriter {
    out: Vec<u8>,
    buf: u64,
    nbits: u32,
}

impl BitWriter {
    fn new(cap: usize) -> Self {
        Self {
            out: Vec::with_capacity(cap),
            buf: 0,
            nbits: 0,
        }
    }

    #[inline]
    fn write_bits(&mut self, val: u32, count: u32) {
        self.buf |= (val as u64) << self.nbits;
        self.nbits += count;
        while self.nbits >= 8 {
            self.out.push((self.buf & 0xFF) as u8);
            self.buf >>= 8;
            self.nbits -= 8;
        }
    }

    #[inline]
    fn write_bits_msb(&mut self, val: u32, count: u32) {
        let rev = (val.reverse_bits()) >> (32 - count);
        self.write_bits(rev, count);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.out.push((self.buf & 0xFF) as u8);
        }
        self.out
    }
}

#[inline]
fn write_fixed_litlen(bw: &mut BitWriter, sym: u32) {
    if sym <= 143 {
        bw.write_bits_msb(0b0011_0000 + sym, 8);
    } else if sym <= 255 {
        bw.write_bits_msb(0b1_1001_0000 + (sym - 144), 9);
    } else if sym <= 279 {
        bw.write_bits_msb(sym - 256, 7);
    } else {
        bw.write_bits_msb(0b1100_0000 + (sym - 280), 8);
    }
}

fn len_to_code(len: usize) -> (u32, u32, u32) {
    if len == 258 {
        return (285, 0, 0);
    }
    for i in (0..28).rev() {
        let base = LEN_BASE[i] as usize;
        if len >= base {
            return (257 + i as u32, (len - base) as u32, LEN_EXTRA[i] as u32);
        }
    }
    (257, 0, 0)
}

fn dist_to_code(dist: usize) -> (u32, u32, u32) {
    for i in (0..30).rev() {
        let base = DIST_BASE[i] as usize;
        if dist >= base {
            return (i as u32, (dist - base) as u32, DIST_EXTRA[i] as u32);
        }
    }
    (0, 0, 0)
}

/// Compress bytes into a standard Zlib stream (`CMF`, `FLG`, DEFLATE block, `ADLER32`).
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    // Zlib header: CM=8 (DEFLATE), CINFO=7 (32K window) => 0x78; FCHECK makes (0x7800 | 0x9C) % 31 == 0
    out.push(0x78);
    out.push(0x9C);

    let mut bw = BitWriter::new(data.len() / 2 + 16);
    // BFINAL=1, BTYPE=01 (fixed Huffman)
    bw.write_bits(1, 1);
    bw.write_bits(1, 2);

    const HASH_BITS: usize = 15;
    const HASH_SIZE: usize = 1 << HASH_BITS;
    const HASH_MASK: usize = HASH_SIZE - 1;
    const MAX_DIST: usize = 32768;
    const MAX_CHAIN: usize = 24;

    let mut head = vec![u32::MAX; HASH_SIZE];
    let mut prev = vec![u32::MAX; data.len().min(MAX_DIST)];

    let hash3 = |i: usize| -> usize {
        let v = ((data[i] as usize) << 10) ^ ((data[i + 1] as usize) << 5) ^ (data[i + 2] as usize);
        (v.wrapping_mul(0x1E35_A7BD)) & HASH_MASK
    };

    let n = data.len();
    let mut i = 0usize;
    while i < n {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;

        if i + 2 < n {
            let h = hash3(i);
            let mut cand = head[h];
            let mut chain = 0usize;
            while cand != u32::MAX && chain < MAX_CHAIN {
                let c = cand as usize;
                let dist = i - c;
                if dist > MAX_DIST || dist == 0 {
                    break;
                }
                if data[c] == data[i] && data[c + 1] == data[i + 1] && data[c + 2] == data[i + 2] {
                    let max_l = (n - i).min(258);
                    let mut l = 3usize;
                    while l < max_l && data[c + l] == data[i + l] {
                        l += 1;
                    }
                    if l > best_len {
                        best_len = l;
                        best_dist = dist;
                        if l >= 128 {
                            break;
                        }
                    }
                }
                cand = prev[c % MAX_DIST];
                chain += 1;
            }
            if !prev.is_empty() {
                prev[i % MAX_DIST] = head[h];
                head[h] = i as u32;
            }
        }

        if best_len >= 3 {
            let (lcode, lextra, lbits) = len_to_code(best_len);
            write_fixed_litlen(&mut bw, lcode);
            if lbits > 0 {
                bw.write_bits(lextra, lbits);
            }
            let (dcode, dextra, dbits) = dist_to_code(best_dist);
            bw.write_bits_msb(dcode, 5);
            if dbits > 0 {
                bw.write_bits(dextra, dbits);
            }
            // Insert intermediate positions into hash table for better compression
            let end = (i + best_len).min(n.saturating_sub(2));
            let mut k = i + 1;
            while k < end {
                let h = hash3(k);
                prev[k % MAX_DIST] = head[h];
                head[h] = k as u32;
                k += 1;
            }
            i += best_len;
        } else {
            write_fixed_litlen(&mut bw, data[i] as u32);
            i += 1;
        }
    }

    // End of block (256)
    write_fixed_litlen(&mut bw, 256);
    out.extend_from_slice(&bw.finish());
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

// ---- Inflate (RFC 1950 + RFC 1951) ----

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    buf: u64,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            buf: 0,
            nbits: 0,
        }
    }

    fn ensure(&mut self, count: u32) -> Result<(), String> {
        while self.nbits < count {
            if self.byte_pos >= self.data.len() {
                return Err("unexpected EOF in deflate stream".into());
            }
            self.buf |= (self.data[self.byte_pos] as u64) << self.nbits;
            self.byte_pos += 1;
            self.nbits += 8;
        }
        Ok(())
    }

    fn read_bits(&mut self, count: u32) -> Result<u32, String> {
        if count == 0 {
            return Ok(0);
        }
        self.ensure(count)?;
        let v = (self.buf & ((1u64 << count) - 1)) as u32;
        self.buf >>= count;
        self.nbits -= count;
        Ok(v)
    }

    fn align_byte(&mut self) {
        let rem = self.nbits % 8;
        if rem > 0 {
            self.buf >>= rem;
            self.nbits -= rem;
        }
    }

    fn read_u16_le(&mut self) -> Result<u16, String> {
        let lo = self.read_bits(8)?;
        let hi = self.read_bits(8)?;
        Ok((lo | (hi << 8)) as u16)
    }
}

struct HuffTable {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl HuffTable {
    fn build(lengths: &[u8]) -> Result<Self, String> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l as usize >= 16 {
                return Err("invalid huffman code length".into());
            }
            if l > 0 {
                counts[l as usize] += 1;
            }
        }
        let mut offsets = [0u16; 16];
        let mut total = 0u16;
        for bits in 1..16 {
            offsets[bits] = total;
            total += counts[bits];
        }
        let mut symbols = vec![0u16; total as usize];
        for (sym, &l) in lengths.iter().enumerate() {
            if l > 0 {
                let idx = offsets[l as usize] as usize;
                offsets[l as usize] += 1;
                symbols[idx] = sym as u16;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader<'_>) -> Result<u16, String> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.read_bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err("invalid huffman code in deflate stream".into())
    }
}

/// Decompress a Zlib stream and validate its Adler-32 checksum.
pub fn zlib_decompress(input: &[u8]) -> Result<Vec<u8>, String> {
    if input.len() < 6 {
        return Err("zlib stream too short".into());
    }
    let cmf = input[0];
    let flg = input[1];
    if (cmf & 0x0F) != 8 {
        return Err("unsupported zlib compression method".into());
    }
    if (((cmf as u16) << 8) | (flg as u16)) % 31 != 0 {
        return Err("invalid zlib header checksum".into());
    }
    if (flg & 0x20) != 0 {
        return Err("preset zlib dictionaries are not supported".into());
    }
    let deflate_slice = &input[2..input.len() - 4];
    let out = inflate(deflate_slice)?;
    let expected = u32::from_be_bytes([
        input[input.len() - 4],
        input[input.len() - 3],
        input[input.len() - 2],
        input[input.len() - 1],
    ]);
    let actual = adler32(&out);
    if actual != expected {
        return Err(format!(
            "zlib adler32 mismatch (expected {expected:#010x}, got {actual:#010x})"
        ));
    }
    Ok(out)
}

fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut br = BitReader::new(data);
    let mut out = Vec::new();
    loop {
        let bfinal = br.read_bits(1)?;
        let btype = br.read_bits(2)?;
        match btype {
            0 => {
                br.align_byte();
                let len = br.read_u16_le()?;
                let nlen = br.read_u16_le()?;
                if len != !nlen {
                    return Err("corrupt stored deflate block length".into());
                }
                for _ in 0..len {
                    out.push(br.read_bits(8)? as u8);
                }
            }
            1 | 2 => {
                let (litlen, dist) = if btype == 1 {
                    let mut ll = [0u8; 288];
                    for b in &mut ll[0..=143] {
                        *b = 8;
                    }
                    for b in &mut ll[144..=255] {
                        *b = 9;
                    }
                    for b in &mut ll[256..=279] {
                        *b = 7;
                    }
                    for b in &mut ll[280..=287] {
                        *b = 8;
                    }
                    let d = [5u8; 32];
                    (HuffTable::build(&ll)?, HuffTable::build(&d)?)
                } else {
                    let hlit = br.read_bits(5)? as usize + 257;
                    let hdist = br.read_bits(5)? as usize + 1;
                    let hclen = br.read_bits(4)? as usize + 4;
                    const CL_ORDER: [usize; 19] = [
                        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                    ];
                    let mut cl_lens = [0u8; 19];
                    for i in 0..hclen {
                        cl_lens[CL_ORDER[i]] = br.read_bits(3)? as u8;
                    }
                    let cl_tab = HuffTable::build(&cl_lens)?;
                    let total = hlit + hdist;
                    let mut lens = Vec::with_capacity(total);
                    while lens.len() < total {
                        let sym = cl_tab.decode(&mut br)?;
                        match sym {
                            0..=15 => lens.push(sym as u8),
                            16 => {
                                let prev = *lens
                                    .last()
                                    .ok_or_else(|| "repeat code with no previous length".to_string())?;
                                let rep = br.read_bits(2)? as usize + 3;
                                lens.resize(lens.len() + rep, prev);
                            }
                            17 => {
                                let rep = br.read_bits(3)? as usize + 3;
                                lens.resize(lens.len() + rep, 0);
                            }
                            18 => {
                                let rep = br.read_bits(7)? as usize + 11;
                                lens.resize(lens.len() + rep, 0);
                            }
                            _ => return Err("invalid code length symbol".into()),
                        }
                    }
                    if lens.len() != total {
                        return Err("dynamic huffman table overflow".into());
                    }
                    (
                        HuffTable::build(&lens[..hlit])?,
                        HuffTable::build(&lens[hlit..])?,
                    )
                };

                loop {
                    let sym = litlen.decode(&mut br)?;
                    if sym < 256 {
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let li = (sym - 257) as usize;
                        if li >= 29 {
                            return Err("invalid length symbol".into());
                        }
                        let length =
                            LEN_BASE[li] as usize + br.read_bits(LEN_EXTRA[li] as u32)? as usize;
                        let di = dist.decode(&mut br)? as usize;
                        if di >= 30 {
                            return Err("invalid distance symbol".into());
                        }
                        let distance =
                            DIST_BASE[di] as usize + br.read_bits(DIST_EXTRA[di] as u32)? as usize;
                        if distance == 0 || distance > out.len() {
                            return Err("invalid LZ77 back-reference distance".into());
                        }
                        for _ in 0..length {
                            let b = out[out.len() - distance];
                            out.push(b);
                        }
                    }
                }
            }
            _ => return Err("invalid deflate block type 3".into()),
        }
        if bfinal != 0 {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_roundtrip() {
        let mut data = Vec::new();
        for i in 0..4096 {
            data.push(((i / 16) % 251) as u8);
        }
        let z = zlib_compress(&data);
        assert!(z.len() < data.len() / 4, "expected strong compression, got {}", z.len());
        let back = zlib_decompress(&z).unwrap();
        assert_eq!(back, data);
    }
}
