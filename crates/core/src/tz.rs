//! Minimal TZif reader: the UTC offset in effect at a given instant.
//!
//! Screen time buckets usage into *local* days, so Linux needs the local UTC
//! offset. Rather than pull in a timezone database crate, this reads the
//! compiled zone file the system already has (`/etc/localtime`, RFC 8536). It
//! is pure (bytes in, offset out) so it is unit-tested with synthetic files.

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    be_u32(b, at).map(|v| v as i32)
}

fn be_i64(b: &[u8], at: usize) -> Option<i64> {
    let s = b.get(at..at + 8)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(i64::from_be_bytes(a))
}

struct Header {
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

fn header(b: &[u8], at: usize) -> Option<(u8, Header)> {
    if b.get(at..at + 4)? != b"TZif" {
        return None;
    }
    let version = *b.get(at + 4)?;
    let c = |i: usize| be_u32(b, at + 20 + i * 4).map(|v| v as usize);
    Some((
        version,
        Header {
            isutcnt: c(0)?,
            isstdcnt: c(1)?,
            leapcnt: c(2)?,
            timecnt: c(3)?,
            typecnt: c(4)?,
            charcnt: c(5)?,
        },
    ))
}

fn block_len(h: &Header, time_size: usize) -> usize {
    h.timecnt * time_size
        + h.timecnt
        + h.typecnt * 6
        + h.charcnt
        + h.leapcnt * (time_size + 4)
        + h.isstdcnt
        + h.isutcnt
}

/// The UTC offset in seconds (local = UTC + offset) at `now_unix`, or `None`
/// when the data is not a valid TZif file.
pub fn offset_at(tzif: &[u8], now_unix: i64) -> Option<i32> {
    let (version, h1) = header(tzif, 0)?;
    // Prefer the 64-bit second block (v2+); fall back to the v1 block.
    let (start, h, time_size) = if version >= b'2' {
        let second = 44 + block_len(&h1, 4);
        let (_, h2) = header(tzif, second)?;
        (second + 44, h2, 8usize)
    } else {
        (44, h1, 4usize)
    };
    if h.typecnt == 0 {
        return None;
    }
    let times_at = start;
    let idx_at = times_at + h.timecnt * time_size;
    let types_at = idx_at + h.timecnt;

    // Index of the last transition at or before `now`.
    let mut chosen: Option<usize> = None;
    for i in 0..h.timecnt {
        let t = if time_size == 8 {
            be_i64(tzif, times_at + i * 8)?
        } else {
            i64::from(be_i32(tzif, times_at + i * 4)?)
        };
        if t <= now_unix {
            chosen = Some(i);
        } else {
            break;
        }
    }
    let type_index = match chosen {
        Some(i) => usize::from(*tzif.get(idx_at + i)?),
        // Before the first transition (or no transitions): the first
        // non-DST type, else type 0, per RFC 8536 section 3.2.
        None => (0..h.typecnt)
            .find(|t| tzif.get(types_at + t * 6 + 4) == Some(&0))
            .unwrap_or(0),
    };
    if type_index >= h.typecnt {
        return None;
    }
    be_i32(tzif, types_at + type_index * 6)
}
