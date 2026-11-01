//! Bounded ZIP + Android binary-XML manifest metadata. This does not validate an
//! APK signature, installability, resources, or whether it is safe to execute.
use crate::ApkMetadata;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const LIMIT: usize = 1024 * 1024;

pub fn metadata(path: &Path) -> ApkMetadata {
    let result = (|| -> Option<ApkMetadata> {
        let mut file = File::open(path).ok()?;
        // Bound the central directory before ZipArchive allocates its index.
        let size = file.metadata().ok()?.len();
        let tail_size = size.min(65_557) as usize;
        file.seek(SeekFrom::End(-(tail_size as i64))).ok()?;
        let mut tail = vec![0; tail_size];
        file.read_exact(&mut tail).ok()?;
        let mut eocd = None;
        for at in (0..tail.len().saturating_sub(21)).rev() {
            if tail.get(at..at + 4) == Some(&[0x50, 0x4b, 0x05, 0x06][..])
                && at
                    .checked_add(22)?
                    .checked_add(u16_at(&tail, at + 20)? as usize)?
                    == tail.len()
            {
                eocd = Some(at);
                break;
            }
        }
        let at = eocd?;
        let entries = u16_at(&tail, at + 10)? as usize;
        let central_size = u32_at(&tail, at + 12)? as u64;
        let central_start = u32_at(&tail, at + 16)? as u64;
        if u16_at(&tail, at + 4)? != 0
            || u16_at(&tail, at + 6)? != 0
            || u16_at(&tail, at + 8)? as usize != entries
            || entries > 10_000
            || central_size > 8 * 1024 * 1024
            || central_start.checked_add(central_size)? > size - tail_size as u64 + at as u64
        {
            return None;
        }
        file.seek(SeekFrom::Start(0)).ok()?;
        let mut zip = zip::ZipArchive::new(file).ok()?;
        if zip.len() > 10_000 {
            return None;
        }
        let entry = zip.by_name("AndroidManifest.xml").ok()?;
        if entry.size() > LIMIT as u64 || entry.compressed_size() > LIMIT as u64 {
            return None;
        }
        let mut bytes = Vec::new();
        entry.take(LIMIT as u64 + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() > LIMIT {
            return None;
        }
        parse(&bytes)
    })();
    result.unwrap_or(ApkMetadata {
        status: "unavailable".into(),
        package_name: None,
        version_name: None,
        version_code: None,
    })
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn parse(b: &[u8]) -> Option<ApkMetadata> {
    if u16_at(b, 0)? != 3 || u32_at(b, 4)? as usize > b.len() {
        return None;
    }
    let end = u32_at(b, 4)? as usize;
    let mut offset = u16_at(b, 2)? as usize;
    let mut strings = Vec::new();
    if offset < 8 {
        return None;
    }
    while offset.checked_add(8)? <= end {
        let ty = u16_at(b, offset)?;
        let header = u16_at(b, offset + 2)? as usize;
        let size = u32_at(b, offset + 4)? as usize;
        if header < 8 || size < header || offset.checked_add(size)? > end {
            return None;
        }
        let chunk = &b[offset..offset + size];
        if ty == 1 {
            strings = parse_strings(chunk)?;
        }
        if ty == 0x102 {
            if header < 16 || size < 36 {
                return None;
            }
            let name = strings.get(u32_at(chunk, 20)? as usize)?;
            if name == "manifest" {
                let attr_start = u16_at(chunk, 24)? as usize;
                let attr_size = u16_at(chunk, 26)? as usize;
                let attr_count = u16_at(chunk, 28)? as usize;
                if attr_size < 20 || attr_count > 512 {
                    return None;
                }
                let mut result = ApkMetadata {
                    status: "available".into(),
                    package_name: None,
                    version_name: None,
                    version_code: None,
                };
                for index in 0..attr_count {
                    let start = 16usize
                        .checked_add(attr_start)?
                        .checked_add(index.checked_mul(attr_size)?)?;
                    let a = chunk.get(start..start.checked_add(attr_size)?)?;
                    let name = strings.get(u32_at(a, 4)? as usize)?;
                    let namespace_index = u32_at(a, 0)?;
                    let android_namespace = strings
                        .get(namespace_index as usize)
                        .is_some_and(|s| s == "http://schemas.android.com/apk/res/android");
                    let raw = u32_at(a, 8)?;
                    let data_type = *a.get(15)?;
                    let value = u32_at(a, 16)?;
                    let decoded = if data_type == 1 || data_type == 2 {
                        None
                    } else if raw != u32::MAX {
                        strings.get(raw as usize).cloned()
                    } else {
                        match data_type {
                            3 => strings.get(value as usize).cloned(),
                            0x10 | 0x11 => Some(value.to_string()),
                            _ => None,
                        }
                    };
                    if decoded
                        .as_ref()
                        .is_some_and(|s| s.len() > 512 || s.chars().any(char::is_control))
                    {
                        continue;
                    }
                    match name.as_str() {
                        "package" if namespace_index == u32::MAX => result.package_name = decoded,
                        "versionName" if android_namespace => result.version_name = decoded,
                        "versionCode" if android_namespace => result.version_code = decoded,
                        _ => {}
                    }
                }
                return Some(result);
            }
        }
        offset += size;
    }
    None
}

fn parse_strings(b: &[u8]) -> Option<Vec<String>> {
    let header = u16_at(b, 2)? as usize;
    if header < 28 {
        return None;
    }
    let count = u32_at(b, 8)? as usize;
    let flags = u32_at(b, 16)?;
    let start = u32_at(b, 20)? as usize;
    if count > 20_000 || header.checked_add(count.checked_mul(4)?)? > b.len() || start > b.len() {
        return None;
    }
    let mut strings = Vec::with_capacity(count);
    let mut decoded_bytes = 0usize;
    for i in 0..count {
        let mut offset = start.checked_add(u32_at(b, header + i * 4)? as usize)?;
        let string = if flags & 0x100 != 0 {
            let _ = length8(b, &mut offset)?;
            let len = length8(b, &mut offset)?;
            let bytes = b.get(offset..offset.checked_add(len)?)?;
            if *b.get(offset.checked_add(len)?)? != 0 {
                return None;
            }
            String::from_utf8(bytes.to_vec()).ok()?
        } else {
            let len = length16(b, &mut offset)?;
            let raw = b.get(offset..offset.checked_add(len.checked_mul(2)?)?)?;
            if u16_at(b, offset.checked_add(len.checked_mul(2)?)?)? != 0 {
                return None;
            }
            let values = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>();
            String::from_utf16(&values).ok()?
        };
        decoded_bytes = decoded_bytes.checked_add(string.len())?;
        if decoded_bytes > 4 * LIMIT {
            return None;
        }
        strings.push(string);
    }
    Some(strings)
}
fn length8(b: &[u8], o: &mut usize) -> Option<usize> {
    let a = *b.get(*o)?;
    *o += 1;
    if a & 0x80 == 0 {
        Some(a as usize)
    } else {
        let z = *b.get(*o)?;
        *o += 1;
        Some((((a & 0x7f) as usize) << 8) | z as usize)
    }
}
fn length16(b: &[u8], o: &mut usize) -> Option<usize> {
    let a = u16_at(b, *o)?;
    *o += 2;
    if a & 0x8000 == 0 {
        Some(a as usize)
    } else {
        let z = u16_at(b, *o)?;
        *o += 2;
        Some((((a & 0x7fff) as usize) << 16) | z as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn reads_real_previous_build_manifest_metadata() {
        // Extracted from the previously built trial APK; no installation result
        // is inferred. The expected identity matches that build's Gradle config.
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../tests/fixtures/v0_2_manifest.b64").trim())
            .unwrap();
        let metadata = parse(&bytes).unwrap();
        assert_eq!(
            metadata.package_name.as_deref(),
            Some("com.kroblus.lanrelay.preview")
        );
        assert_eq!(metadata.version_name.as_deref(), Some("0.2.0-preview"));
        assert_eq!(metadata.version_code.as_deref(), Some("2"));
    }
    #[test]
    fn malformed_manifests_are_unknown() {
        for bytes in [
            &[][..],
            &[3, 0, 8, 0, 255, 255, 255, 255][..],
            b"not an apk",
        ] {
            assert!(parse(bytes).is_none());
        }
    }
    #[test]
    fn string_lengths_reject_truncated_encodings() {
        assert_eq!(length8(&[0x80], &mut 0), None);
        assert_eq!(length16(&[0, 0x80], &mut 0), None);
    }
}
