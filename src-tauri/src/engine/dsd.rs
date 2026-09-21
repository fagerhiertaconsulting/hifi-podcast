//! DSF / DSDIFF (.dsf / .dff) demux. Output is interleaved DSD bytes (8 bits/channel).
//! No analog PCM conversion. Original file is never rewritten.

use super::formats::{classify_magic, FormatKind};
use super::EngineError;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// SACD analog-ish mute pattern (not 0x00, which is DC).
pub const DSD_MUTE: u8 = 0x69;
pub const DOP_MARKER_A: u8 = 0x05;
pub const DOP_MARKER_B: u8 = 0xFA;

const BIT_REVERSE: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = (i as u8).reverse_bits();
        i += 1;
    }
    t
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsdContainer {
    Dsf,
    Dff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitOrder {
    LsbFirst,
    MsbFirst,
}

#[derive(Debug, Clone)]
pub struct DsdSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
    pub container: DsdContainer,
    pub bit_order: BitOrder,
    pub rate_label: String,
    pub codec: String,
}

pub struct DsdReader {
    file: File,
    spec: DsdSpec,
    data_start: u64,
    data_end: u64,
    /// DSF: bytes per channel in one interleaved block. DFF: 1 (byte-interleaved).
    block_size: usize,
    cursor: u64,
    leftover: Vec<u8>,
    eof: bool,
}

impl DsdReader {
    pub fn open(path: &Path) -> Result<Self, EngineError> {
        let mut header = [0u8; 16];
        {
            let mut f = File::open(path)?;
            let n = f.read(&mut header)?;
            if n < 4 {
                return Err(EngineError::msg("Filen är för kort för DSD."));
            }
        }
        match classify_magic(&header, Some(path)) {
            FormatKind::Dsd => {}
            _ => {
                return Err(EngineError::msg(
                    "Filen är inte DSD (.dsf/.dff). Native-motorn lämnar originalet orört.",
                ));
            }
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "iso" {
            return Err(EngineError::msg(
                "SACD-ISO rippning ingår inte. Använd .dsf/.dff (inte avbildningen). Filen rörs inte.",
            ));
        }
        if &header[..4] == b"DSD " {
            Self::open_dsf(path)
        } else if &header[..4] == b"FRM8" {
            Self::open_dff(path)
        } else {
            Err(EngineError::msg(
                "DSD-magi saknas. Bara .dsf (DSD ) och .dff (FRM8) avkodas. Filen rörs inte.",
            ))
        }
    }

    pub fn spec(&self) -> &DsdSpec {
        &self.spec
    }

    pub fn is_eof(&self) -> bool {
        self.eof && self.leftover.is_empty()
    }

    /// Interleaved DSD bytes (L,R,…). Empty = EOF.
    pub fn read_interleaved_bytes(&mut self, min_bytes: usize) -> Result<Vec<u8>, EngineError> {
        let mut out = Vec::new();
        if !self.leftover.is_empty() {
            out.append(&mut self.leftover);
        }
        let ch = self.spec.channels as usize;
        let block = self.block_size.max(1) * ch.max(1);
        while out.len() < min_bytes && !self.eof {
            let remaining = self.data_end.saturating_sub(self.cursor);
            if remaining == 0 {
                self.eof = true;
                break;
            }
            let take = (remaining as usize).min(block);
            let mut buf = vec![0u8; take];
            self.file.seek(SeekFrom::Start(self.cursor))?;
            let n = self.file.read(&mut buf)?;
            if n == 0 {
                self.eof = true;
                break;
            }
            buf.truncate(n);
            self.cursor += n as u64;
            if self.spec.container == DsdContainer::Dsf && ch > 1 && self.block_size > 0 {
                out.extend_from_slice(&deinterleave_dsf_block(&buf, ch, self.block_size));
            } else {
                out.extend_from_slice(&buf);
            }
        }
        Ok(out)
    }

    pub fn unread(&mut self, bytes: &[u8]) {
        let mut rest = bytes.to_vec();
        rest.append(&mut self.leftover);
        self.leftover = rest;
    }

    pub fn seek_ms(&mut self, ms: u64) -> Result<u64, EngineError> {
        let rate = self.spec.sample_rate.max(1) as u128;
        let ch = self.spec.channels.max(1) as u128;
        let bits = ms as u128 * rate / 1000;
        let bytes_per_ch = bits / 8;
        let align = self.block_size.max(1) as u128;
        let aligned = (bytes_per_ch / align) * align;
        let payload = aligned * ch;
        let max_payload = self.data_end.saturating_sub(self.data_start);
        let payload = (payload as u64).min(max_payload);
        self.cursor = self.data_start + payload;
        self.leftover.clear();
        self.eof = self.cursor >= self.data_end;
        let actual_bits = (payload as u128 / ch) * 8;
        Ok((actual_bits * 1000 / rate) as u64)
    }

    fn open_dsf(path: &Path) -> Result<Self, EngineError> {
        let mut file = File::open(path)?;
        let mut dsd = [0u8; 28];
        file.read_exact(&mut dsd)?;
        if &dsd[0..4] != b"DSD " {
            return Err(EngineError::msg("Ogiltig DSF-header (DSD )."));
        }
        let mut fmt_hdr = [0u8; 12];
        file.read_exact(&mut fmt_hdr)?;
        if &fmt_hdr[0..4] != b"fmt " {
            return Err(EngineError::msg("Ogiltig DSF: saknar fmt-chunk."));
        }
        let fmt_chunk_size = u64::from_le_bytes(fmt_hdr[4..12].try_into().unwrap());
        let fmt_body_len = fmt_chunk_size.saturating_sub(12) as usize;
        if fmt_body_len < 40 {
            return Err(EngineError::msg("Ogiltig DSF fmt-chunk."));
        }
        let mut fmt = vec![0u8; fmt_body_len];
        file.read_exact(&mut fmt)?;
        let format_id = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
        if format_id != 0 {
            return Err(EngineError::msg(
                "DSF format-ID är inte rå-DSD. Filen avkodas inte. Originalet rörs inte.",
            ));
        }
        let channels = u32::from_le_bytes(fmt[12..16].try_into().unwrap()) as u16;
        let sample_rate = u32::from_le_bytes(fmt[16..20].try_into().unwrap());
        let bits_per_sample = u32::from_le_bytes(fmt[20..24].try_into().unwrap());
        let sample_count = u64::from_le_bytes(fmt[24..32].try_into().unwrap());
        let block_size = u32::from_le_bytes(fmt[32..36].try_into().unwrap()) as usize;
        if channels == 0 || sample_rate == 0 || block_size == 0 {
            return Err(EngineError::msg("DSF har ogiltig kanal-/hastighetsdata."));
        }
        if bits_per_sample != 1 && bits_per_sample != 8 {
            return Err(EngineError::msg(format!(
                "DSF bits/sample {bits_per_sample} stöds inte (väntat 1 eller 8)."
            )));
        }
        let mut data_hdr = [0u8; 12];
        file.read_exact(&mut data_hdr)?;
        if &data_hdr[0..4] != b"data" {
            return Err(EngineError::msg("Ogiltig DSF: saknar data-chunk."));
        }
        let data_chunk_size = u64::from_le_bytes(data_hdr[4..12].try_into().unwrap());
        let data_start = file.stream_position()?;
        let payload = data_chunk_size.saturating_sub(12);
        let data_end = data_start + payload;
        let duration_ms = if sample_rate > 0 {
            (sample_count as u128 * 1000 / sample_rate as u128) as u64
        } else {
            0
        };
        let spec = DsdSpec {
            sample_rate,
            channels,
            duration_ms,
            container: DsdContainer::Dsf,
            bit_order: BitOrder::LsbFirst,
            rate_label: dsd_rate_label(sample_rate),
            codec: "DSD".into(),
        };
        Ok(Self {
            file,
            spec,
            data_start,
            data_end,
            block_size,
            cursor: data_start,
            leftover: Vec::new(),
            eof: payload == 0,
        })
    }

    fn open_dff(path: &Path) -> Result<Self, EngineError> {
        let mut file = File::open(path)?;
        let mut hdr = [0u8; 16];
        file.read_exact(&mut hdr)?;
        if &hdr[0..4] != b"FRM8" || &hdr[12..16] != b"DSD " {
            return Err(EngineError::msg(
                "Ogiltig DSDIFF-header (FRM8/DSD). DST-komprimerad DFF stöds inte här.",
            ));
        }
        let form_end = 12 + u64::from_be_bytes(hdr[4..12].try_into().unwrap());
        let mut sample_rate = 0u32;
        let mut channels = 0u16;
        let mut compressed = false;
        let mut data_start = 0u64;
        let mut data_end = 0u64;
        let mut found_data = false;

        while file.stream_position()? + 12 <= form_end {
            let mut id = [0u8; 4];
            if file.read_exact(&mut id).is_err() {
                break;
            }
            let mut sz = [0u8; 8];
            file.read_exact(&mut sz)?;
            let size = u64::from_be_bytes(sz);
            let chunk_data = file.stream_position()?;
            let next = align2(chunk_data + size);
            match &id {
                b"PROP" => {
                    let mut prop = [0u8; 4];
                    file.read_exact(&mut prop)?;
                    if &prop == b"SND " {
                        parse_dff_prop(
                            &mut file,
                            chunk_data + 4,
                            chunk_data + size,
                            &mut sample_rate,
                            &mut channels,
                            &mut compressed,
                        )?;
                    }
                }
                b"DSD " => {
                    data_start = chunk_data;
                    data_end = chunk_data + size;
                    found_data = true;
                }
                b"DST " => {
                    return Err(EngineError::msg(
                        "DSDIFF DST-komprimering avkodas inte. Exportera okomprimerad DFF/DSF. Filen rörs inte.",
                    ));
                }
                _ => {}
            }
            file.seek(SeekFrom::Start(next.min(form_end)))?;
            if file.stream_position()? >= form_end {
                break;
            }
        }

        if compressed {
            return Err(EngineError::msg(
                "DSDIFF är DST-komprimerad. Native-motorn avkodar bara rå DSD. Filen rörs inte.",
            ));
        }
        if !found_data || sample_rate == 0 || channels == 0 {
            return Err(EngineError::msg(
                "DSDIFF saknar DSD-data, samplingsfrekvens eller kanaler.",
            ));
        }
        let payload = data_end.saturating_sub(data_start);
        let duration_ms = if sample_rate > 0 && channels > 0 {
            let bits = payload as u128 * 8 / channels as u128;
            (bits * 1000 / sample_rate as u128) as u64
        } else {
            0
        };
        let spec = DsdSpec {
            sample_rate,
            channels,
            duration_ms,
            container: DsdContainer::Dff,
            bit_order: BitOrder::MsbFirst,
            rate_label: dsd_rate_label(sample_rate),
            codec: "DSD".into(),
        };
        file.seek(SeekFrom::Start(data_start))?;
        Ok(Self {
            file,
            spec,
            data_start,
            data_end,
            block_size: 1,
            cursor: data_start,
            leftover: Vec::new(),
            eof: payload == 0,
        })
    }
}

fn align2(pos: u64) -> u64 {
    (pos + 1) & !1
}

fn parse_dff_prop(
    file: &mut File,
    start: u64,
    end: u64,
    sample_rate: &mut u32,
    channels: &mut u16,
    compressed: &mut bool,
) -> Result<(), EngineError> {
    file.seek(SeekFrom::Start(start))?;
    while file.stream_position()? + 12 <= end {
        let mut id = [0u8; 4];
        file.read_exact(&mut id)?;
        let mut sz = [0u8; 8];
        file.read_exact(&mut sz)?;
        let size = u64::from_be_bytes(sz);
        let data_pos = file.stream_position()?;
        let next = align2(data_pos + size);
        match &id {
            b"FS  " if size >= 4 => {
                let mut r = [0u8; 4];
                file.read_exact(&mut r)?;
                *sample_rate = u32::from_be_bytes(r);
            }
            b"CHNL" if size >= 2 => {
                let mut n = [0u8; 2];
                file.read_exact(&mut n)?;
                *channels = u16::from_be_bytes(n);
            }
            b"CMPR" if size >= 4 => {
                let mut t = [0u8; 4];
                file.read_exact(&mut t)?;
                if &t == b"DST " {
                    *compressed = true;
                }
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(next.min(end)))?;
    }
    Ok(())
}

/// DSF stores [ch0 block][ch1 block]… — make byte-interleaved L,R,L,R.
fn deinterleave_dsf_block(buf: &[u8], channels: usize, block_size: usize) -> Vec<u8> {
    if channels <= 1 || block_size == 0 {
        return buf.to_vec();
    }
    let frames = (buf.len() / channels).min(block_size);
    if frames == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for ch in 0..channels {
            let idx = ch * block_size + i;
            out.push(*buf.get(idx).unwrap_or(&DSD_MUTE));
        }
    }
    out
}

pub fn dsd_rate_label(sample_rate: u32) -> String {
    match sample_rate {
        2_822_400 | 3_072_000 => "DSD64".into(),
        5_644_800 | 6_144_000 => "DSD128".into(),
        11_289_600 | 12_288_000 => "DSD256".into(),
        22_579_200 | 24_576_000 => "DSD512".into(),
        other => format!("DSD {other} Hz"),
    }
}

/// PCM sample rate used by DoP (16 DSD bits per 24-bit frame).
pub fn dop_pcm_rate(dsd_hz: u32) -> u32 {
    dsd_hz / 16
}

/// ASIO native DSD often uses 1 byte per sample (8 DSD bits).
pub fn native_dsd_byte_rate(dsd_hz: u32) -> u32 {
    dsd_hz / 8
}

pub fn reverse_bits_byte(b: u8) -> u8 {
    BIT_REVERSE[b as usize]
}

#[allow(dead_code)]
pub fn maybe_reverse_bytes(bytes: &mut [u8], file_lsb: bool, want_lsb: bool) {
    if file_lsb != want_lsb {
        for b in bytes {
            *b = reverse_bits_byte(*b);
        }
    }
}

pub fn to_stereo_bytes(interleaved: &[u8], channels: u16) -> Vec<u8> {
    match channels {
        0 => Vec::new(),
        1 => interleaved.iter().flat_map(|b| [*b, *b]).collect(),
        2 => interleaved.to_vec(),
        n => {
            let n = n as usize;
            let frames = interleaved.len() / n;
            let mut out = Vec::with_capacity(frames * 2);
            for frame in 0..frames {
                let base = frame * n;
                out.push(interleaved[base]);
                out.push(if n > 1 {
                    interleaved[base + 1]
                } else {
                    interleaved[base]
                });
            }
            out
        }
    }
}

/// Pack one stereo-or-mono DSD byte frame pair into DoP 24-bit LE samples.
/// `dsd` is interleaved channel bytes; consumes 2 bytes/channel per PCM frame.
/// Returns (pcm_le_bytes, leftover_dsd, next_marker_is_a).
pub fn pack_dop_i24(
    dsd: &[u8],
    channels: usize,
    mut marker_a: bool,
    reverse: bool,
) -> (Vec<u8>, Vec<u8>, bool) {
    let ch = channels.max(1);
    let frames = dsd.len() / (ch * 2);
    let used = frames * ch * 2;
    let mut out = Vec::with_capacity(frames * ch * 3);
    for f in 0..frames {
        let marker = if marker_a { DOP_MARKER_A } else { DOP_MARKER_B };
        marker_a = !marker_a;
        for c in 0..ch {
            let i0 = f * 2 * ch + c;
            let i1 = i0 + ch;
            let mut b0 = dsd[i0];
            let mut b1 = dsd[i1];
            if reverse {
                b0 = reverse_bits_byte(b0);
                b1 = reverse_bits_byte(b1);
            }
            // 24-bit: (marker << 16) | (first_in_time << 8) | second_in_time — LE bytes
            out.push(b1);
            out.push(b0);
            out.push(marker);
        }
    }
    (out, dsd[used..].to_vec(), marker_a)
}

#[allow(dead_code)]
pub fn pack_dop_i32_left(dsd: &[u8], channels: usize, marker_a: bool, reverse: bool) -> (Vec<u8>, Vec<u8>, bool) {
    let (i24, rest, next) = pack_dop_i24(dsd, channels, marker_a, reverse);
    let mut out = Vec::with_capacity(i24.len() / 3 * 4);
    for chunk in i24.chunks_exact(3) {
        out.extend_from_slice(&[0, chunk[0], chunk[1], chunk[2]]);
    }
    (out, rest, next)
}

#[cfg(test)]
pub fn write_test_dsf(path: &Path, sample_rate: u32, channels: u16, bytes_per_ch: usize, fill: u8) {
    let block = 16usize;
    assert_eq!(bytes_per_ch % block, 0);
    let n_blocks = bytes_per_ch / block;
    let payload = (block * channels as usize) * n_blocks;
    let sample_count = (bytes_per_ch as u64) * 8;
    let fmt_chunk_size: u64 = 52;
    let data_chunk_size: u64 = 12 + payload as u64;
    let file_size: u64 = 28 + fmt_chunk_size + data_chunk_size;
    let mut v = Vec::new();
    v.extend(b"DSD ");
    v.extend(28u64.to_le_bytes());
    v.extend(file_size.to_le_bytes());
    v.extend(0u64.to_le_bytes());
    v.extend(b"fmt ");
    v.extend(fmt_chunk_size.to_le_bytes());
    v.extend(1u32.to_le_bytes()); // version
    v.extend(0u32.to_le_bytes()); // raw DSD
    v.extend(2u32.to_le_bytes()); // channel type stereo-ish
    v.extend((channels as u32).to_le_bytes());
    v.extend(sample_rate.to_le_bytes());
    v.extend(1u32.to_le_bytes());
    v.extend(sample_count.to_le_bytes());
    v.extend((block as u32).to_le_bytes());
    v.extend(0u32.to_le_bytes());
    v.extend(b"data");
    v.extend(data_chunk_size.to_le_bytes());
    for _ in 0..n_blocks {
        for ch in 0..channels as usize {
            let b = fill.wrapping_add(ch as u8);
            v.extend(std::iter::repeat(b).take(block));
        }
    }
    std::fs::write(path, v).unwrap();
}

#[cfg(test)]
pub fn write_test_dff(path: &Path, sample_rate: u32, channels: u16, bytes_per_ch: usize, fill: u8) {
    let payload: Vec<u8> = (0..bytes_per_ch)
        .flat_map(|_| (0..channels).map(|ch| fill.wrapping_add(ch as u8)))
        .collect();
    let mut prop_body = Vec::new();
    prop_body.extend(b"SND ");
    // FS
    prop_body.extend(b"FS  ");
    prop_body.extend(4u64.to_be_bytes());
    prop_body.extend(sample_rate.to_be_bytes());
    // CHNL
    let chnl_size = 2 + 4 * channels as u64;
    prop_body.extend(b"CHNL");
    prop_body.extend(chnl_size.to_be_bytes());
    prop_body.extend(channels.to_be_bytes());
    for i in 0..channels {
        if i == 0 {
            prop_body.extend(b"SLFT");
        } else {
            prop_body.extend(b"SRGT");
        }
    }
    if chnl_size % 2 == 1 {
        prop_body.push(0);
    }
    // CMPR
    prop_body.extend(b"CMPR");
    prop_body.extend(4u64.to_be_bytes());
    prop_body.extend(b"DSD ");

    let mut form = Vec::new();
    form.extend(b"DSD ");
    form.extend(b"PROP");
    form.extend((prop_body.len() as u64).to_be_bytes());
    form.extend(&prop_body);
    if prop_body.len() % 2 == 1 {
        form.push(0);
    }
    form.extend(b"DSD ");
    form.extend((payload.len() as u64).to_be_bytes());
    form.extend(&payload);
    if payload.len() % 2 == 1 {
        form.push(0);
    }

    let mut v = Vec::new();
    v.extend(b"FRM8");
    v.extend((form.len() as u64).to_be_bytes());
    v.extend(form);
    std::fs::write(path, v).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsf_roundtrip_not_silent_and_deblocked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.dsf");
        write_test_dsf(&path, 2_822_400, 2, 16 * 400, 0xA5);
        let mut r = DsdReader::open(&path).unwrap();
        assert_eq!(r.spec().rate_label, "DSD64");
        assert_eq!(r.spec().bit_order, BitOrder::LsbFirst);
        assert_eq!(r.spec().channels, 2);
        assert!(r.spec().duration_ms > 0);
        let bytes = r.read_interleaved_bytes(32).unwrap();
        assert!(bytes.len() >= 32);
        assert_eq!(bytes[0], 0xA5);
        assert_eq!(bytes[1], 0xA6);
        assert!(bytes.iter().any(|b| *b != 0));
    }

    #[test]
    fn dff_roundtrip_msb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.dff");
        write_test_dff(&path, 11_289_600, 2, 8, 0x3C);
        let mut r = DsdReader::open(&path).unwrap();
        assert_eq!(r.spec().rate_label, "DSD256");
        assert_eq!(r.spec().bit_order, BitOrder::MsbFirst);
        let bytes = r.read_interleaved_bytes(8).unwrap();
        assert_eq!(bytes[0], 0x3C);
        assert_eq!(bytes[1], 0x3D);
    }

    #[test]
    fn dop_markers_alternate_and_never_claim_pcm_identity() {
        let dsd = [0xA5, 0x5A, 0xA5, 0x5A];
        let (packed, rest, next_a) = pack_dop_i24(&dsd, 2, true, false);
        assert!(rest.is_empty());
        assert!(!next_a);
        assert_eq!(packed.len(), 6);
        assert_eq!(packed[2], DOP_MARKER_A);
        assert_eq!(packed[5], DOP_MARKER_A); // same PCM frame, both channels
        let dsd2 = [0xA5, 0x5A, 0xA5, 0x5A, 0x11, 0x22, 0x11, 0x22];
        let (packed2, _, next2) = pack_dop_i24(&dsd2, 2, true, false);
        assert_eq!(packed2.len(), 12);
        assert_eq!(packed2[2], DOP_MARKER_A);
        assert_eq!(packed2[8], DOP_MARKER_B);
        assert!(next2);
        assert_eq!(packed[1], 0xA5);
        assert_eq!(packed[0], 0xA5);
    }

    #[test]
    fn bit_reverse_table() {
        assert_eq!(reverse_bits_byte(0b1000_0001), 0b1000_0001);
        assert_eq!(reverse_bits_byte(0b0000_0001), 0b1000_0000);
        assert_eq!(reverse_bits_byte(0xA5), (0xA5u8).reverse_bits());
    }

    #[test]
    fn dest_iso_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sacd.iso");
        std::fs::write(&path, b"DSD fake").unwrap();
        let err = DsdReader::open(&path).err().expect("iso refused");
        assert!(err.to_string().contains("ISO"), "{err}");
    }

    #[test]
    fn seek_rewinds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.dsf");
        write_test_dsf(&path, 2_822_400, 2, 64, 0x11);
        let mut r = DsdReader::open(&path).unwrap();
        let _ = r.read_interleaved_bytes(64).unwrap();
        r.seek_ms(0).unwrap();
        let b = r.read_interleaved_bytes(4).unwrap();
        assert_eq!(b[0], 0x11);
    }

    #[test]
    fn dop_rate_dsd256() {
        assert_eq!(dop_pcm_rate(11_289_600), 705_600);
        assert_eq!(native_dsd_byte_rate(11_289_600), 1_411_200);
    }
}
