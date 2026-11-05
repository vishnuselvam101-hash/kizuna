//! Minimal, bounds-checked ZIP central-directory reader used only to vet XLSX uploads
//! (entry names, declared sizes, compression ratio). It never decompresses anything.

pub struct Entry {
    pub name: String,
    pub compressed: u64,
    pub uncompressed: u64,
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
        let name_start = p + 46;
        let name = b.get(name_start..name_start + n).ok_or(ZipError::Malformed)?;
        out.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            compressed: comp as u64,
            uncompressed: uncomp as u64,
        });
        p = name_start + n + x + c;
        if p > end {
            return Err(ZipError::Malformed);
        }
    }
    Ok(out)
}
