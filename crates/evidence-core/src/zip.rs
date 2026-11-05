//! Minimal, bounds-checked ZIP central-directory reader used only to vet XLSX uploads
//! (entry names, declared sizes, compression ratio). The only entries it ever decompresses are small package
//! relationship parts (`*.rels`), with a hard output cap, to find external relationships (red-team 2026-11-05 F4).

pub struct Entry {
    pub name: String,
    pub compressed: u64,
    pub uncompressed: u64,
    /// 0 = stored, 8 = deflate; anything else is unreadable here.
    pub method: u16,
    /// Offset of the entry's local file header.
    pub local_offset: u64,
}

/// The entry's bytes (stored or deflate only), or `None` when unreadable or larger than `max_out` bytes.
pub fn read_entry(b: &[u8], e: &Entry, max_out: usize) -> Option<Vec<u8>> {
    let p = usize::try_from(e.local_offset).ok()?;
    if u32_at(b, p)? != 0x0403_4b50 {
        return None;
    }
    let n = u16_at(b, p + 26)? as usize;
    let x = u16_at(b, p + 28)? as usize;
    let start = p.checked_add(30)?.checked_add(n)?.checked_add(x)?;
    let data = b.get(start..start.checked_add(usize::try_from(e.compressed).ok()?)?)?;
    match e.method {
        0 => (data.len() <= max_out).then(|| data.to_vec()),
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, max_out).ok(),
        _ => None,
    }
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i.checked_add(2)?)?.try_into().ok()?))
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i.checked_add(4)?)?.try_into().ok()?))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ZipError {
    NoEndRecord,
    Zip64Unsupported,
    Malformed,
    TooManyEntries,
}

pub fn central_directory(b: &[u8], max_entries: usize) -> Result<Vec<Entry>, ZipError> {
    // End of central directory: 22 bytes + comment (<= 65535), search backwards.
    let min = b.len().saturating_sub(22 + 65_535);
    let eocd = (min..=b.len().saturating_sub(22))
        .rev()
        .find(|&i| u32_at(b, i) == Some(0x0605_4b50))
        .ok_or(ZipError::NoEndRecord)?;
    let total = u16_at(b, eocd + 10).ok_or(ZipError::Malformed)? as usize;
    let cd_size = u32_at(b, eocd + 12).ok_or(ZipError::Malformed)?;
    let cd_off = u32_at(b, eocd + 16).ok_or(ZipError::Malformed)?;
    if total == 0xFFFF || cd_size == u32::MAX || cd_off == u32::MAX {
        return Err(ZipError::Zip64Unsupported);
    }
    if total > max_entries {
        return Err(ZipError::TooManyEntries);
    }
    let mut p = cd_off as usize;
    let end = p.checked_add(cd_size as usize).ok_or(ZipError::Malformed)?;
    if end > eocd {
        return Err(ZipError::Malformed);
    }
    let mut out = Vec::with_capacity(total);
    for _ in 0..total {
        if u32_at(b, p) != Some(0x0201_4b50) {
            return Err(ZipError::Malformed);
        }
        let comp = u32_at(b, p + 20).ok_or(ZipError::Malformed)?;
        let uncomp = u32_at(b, p + 24).ok_or(ZipError::Malformed)?;
        if comp == u32::MAX || uncomp == u32::MAX {
            return Err(ZipError::Zip64Unsupported);
        }
        let n = u16_at(b, p + 28).ok_or(ZipError::Malformed)? as usize;
        let x = u16_at(b, p + 30).ok_or(ZipError::Malformed)? as usize;
        let c = u16_at(b, p + 32).ok_or(ZipError::Malformed)? as usize;
        let method = u16_at(b, p + 10).ok_or(ZipError::Malformed)?;
        let local = u32_at(b, p + 42).ok_or(ZipError::Malformed)?;
        let name_start = p + 46;
        let name = b.get(name_start..name_start + n).ok_or(ZipError::Malformed)?;
        out.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            compressed: comp as u64,
            uncompressed: uncomp as u64,
            method,
            local_offset: local as u64,
        });
        p = name_start + n + x + c;
        if p > end {
            return Err(ZipError::Malformed);
        }
    }
    Ok(out)
}
