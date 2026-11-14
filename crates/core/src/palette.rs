//! Accent colours from album art, so the Music panel can glow in the cover's
//! colours. Pure function over RGBA bytes; computed once per track change.

/// Two dominant, non-grey colours of a square RGBA8 image (`side` × `side`),
/// heaviest first. `None` when the image is all greys, blacks or whites (the
/// caller then uses the default accent).
pub fn dominant_pair(rgba: &[u8], side: u32) -> Option<([u8; 3], [u8; 3])> {
    let side = side as usize;
    if side == 0 || rgba.len() < side * side * 4 {
        return None;
    }
    // (count, sum r, sum g, sum b) per 4-bit-per-channel bucket.
    let mut buckets = vec![(0u32, 0u32, 0u32, 0u32); 4096];
    let step = (side / 32).max(1);
    let mut y = 0;
    while y < side {
        let mut x = 0;
        while x < side {
            let i = (y * side + x) * 4;
            let (r, g, b) = (rgba[i], rgba[i + 1], rgba[i + 2]);
            let hi = r.max(g).max(b);
            let lo = r.min(g).min(b);
            if hi - lo >= 24 && hi >= 32 && lo <= 235 {
                let k =
                    (usize::from(r >> 4) << 8) | (usize::from(g >> 4) << 4) | usize::from(b >> 4);
                let e = &mut buckets[k];
                e.0 += 1;
                e.1 += u32::from(r);
                e.2 += u32::from(g);
                e.3 += u32::from(b);
            }
            x += step;
        }
        y += step;
    }
    let mut ranked: Vec<(u32, [u8; 3])> = buckets
        .iter()
        .filter(|e| e.0 > 0)
        .map(|e| {
            (
                e.0,
                [(e.1 / e.0) as u8, (e.2 / e.0) as u8, (e.3 / e.0) as u8],
            )
        })
        .collect();
    ranked.sort_by_key(|e| std::cmp::Reverse(e.0));
    let first = ranked.first()?.1;
    let distance = |a: [u8; 3], b: [u8; 3]| -> i32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (i32::from(*x) - i32::from(y)).abs())
            .sum()
    };
    let second = ranked
        .iter()
        .map(|e| e.1)
        .find(|c| distance(*c, first) > 48)
        .unwrap_or_else(|| first.map(|v| (u16::from(v) + (255 - u16::from(v)) * 35 / 100) as u8));
    Some((first, second))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(side: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity((side * side * 4) as usize);
        for y in 0..side {
            for x in 0..side {
                let [r, g, b] = f(x, y);
                v.extend_from_slice(&[r, g, b, 255]);
            }
        }
        v
    }

    fn near(a: [u8; 3], b: [u8; 3]) -> bool {
        a.iter()
            .zip(b)
            .all(|(x, y)| (i16::from(*x) - i16::from(y)).abs() <= 16)
    }

    #[test]
    fn two_coloured_cover_yields_both_colours() {
        let img = image(128, |x, _| {
            if x < 64 {
                [0xFF, 0x7A, 0x59]
            } else {
                [0xC4, 0x31, 0x7A]
            }
        });
        let (a, b) = dominant_pair(&img, 128).unwrap_or_else(|| panic!("no accent"));
        let want = ([0xFF, 0x7A, 0x59], [0xC4, 0x31, 0x7A]);
        assert!(
            (near(a, want.0) && near(b, want.1)) || (near(a, want.1) && near(b, want.0)),
            "{a:?} {b:?}"
        );
    }

    #[test]
    fn grey_cover_has_no_accent() {
        let img = image(64, |_, _| [0x80, 0x80, 0x80]);
        assert_eq!(dominant_pair(&img, 64), None);
    }

    #[test]
    fn bad_input_is_handled() {
        assert_eq!(dominant_pair(&[], 0), None);
        assert_eq!(
            dominant_pair(&[1, 2, 3], 16),
            None,
            "too short for the side"
        );
    }
}
