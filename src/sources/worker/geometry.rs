use super::{Error, records::Page};

/// Integer render dimensions at at most 150 dpi, within the import image rules.
///
/// # Errors
/// Refuses invalid sizes or aspect ratios without a representable bounded integer image.
pub fn pixel_size(width: f64, height: f64) -> Result<(u32, u32), Error> {
    if !(1.0..=4000.0).contains(&width) || !(1.0..=4000.0).contains(&height) {
        return Err(Error::Unsupported);
    }
    let scale = (150.0_f64 / 72.0)
        .min(4096.0 / width)
        .min(8192.0 / height)
        .min((12_000_000.0 / (width * height)).sqrt());
    let max_w = floor_u32((width * scale).ceil().min(4096.0));
    let max_h = floor_u32((height * scale).ceil().min(8192.0));
    let mut best = None;
    let mut area = 0;
    for h in 1..=max_h {
        let w = floor_u32((width / height * f64::from(h)).round()).max(1);
        if w <= max_w
            && u64::from(w) * u64::from(h) <= 12_000_000
            && super::super::import::image_aspect_matches(w, h, width, height)
            && w * h > area
        {
            best = Some((w, h));
            area = w * h;
        }
    }
    best.ok_or(Error::Unsupported)
}

// Inputs to this helper are finite, nonnegative and below u32::MAX.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn floor_u32(value: f64) -> u32 {
    value as u32
}

pub(super) fn linear(rotate: u16, unit: f64) -> Result<[f64; 4], Error> {
    Ok(match rotate {
        0 => [unit, 0.0, 0.0, -unit],
        90 => [0.0, unit, unit, 0.0],
        180 => [-unit, 0.0, 0.0, unit],
        270 => [0.0, -unit, -unit, 0.0],
        _ => return Err(Error::Unsupported),
    })
}

pub(super) fn transformed_box(visible: [f64; 4], m: [f64; 6]) -> [f64; 4] {
    let mut out = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for x in [visible[0], visible[2]] {
        for y in [visible[1], visible[3]] {
            let xx = m[0] * x + m[2] * y + m[4];
            let yy = m[1] * x + m[3] * y + m[5];
            out[0] = out[0].min(xx);
            out[1] = out[1].min(yy);
            out[2] = out[2].max(xx);
            out[3] = out[3].max(yy);
        }
    }
    out
}

pub(super) fn validate_page(page: &Page) -> Result<(), Error> {
    let v = page.visible;
    if page.n == 0
        || page.n > 2000
        || !(1.0..=4000.0).contains(&page.width)
        || !(1.0..=4000.0).contains(&page.height)
        || !(0.1..=10.0).contains(&page.user_unit)
        || v.iter()
            .any(|n| !n.is_finite() || n.abs() > 14400.0 * page.user_unit)
        || v[0] >= v[2]
        || v[1] >= v[3]
        || page
            .transform
            .iter()
            .any(|n| !n.is_finite() || n.abs() > 100_000.0)
    {
        return Err(Error::Unsupported);
    }
    let expected = linear(page.rotate, page.user_unit)?;
    if expected
        .iter()
        .zip(page.transform)
        .any(|(a, b)| (a - b).abs() > 1e-6 * page.user_unit)
        || page.transform[0] * page.transform[3] - page.transform[1] * page.transform[2] == 0.0
    {
        return Err(Error::Unsupported);
    }
    let bounds = transformed_box(v, page.transform);
    if bounds
        .iter()
        .zip([0.0, 0.0, page.width, page.height])
        .any(|(a, b)| (a - b).abs() > 0.5)
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}

/// Union of clipped image bounding boxes; rotated image coverage is an estimate.
pub(super) fn image_coverage(boxes: &[[f64; 4]]) -> f64 {
    let mut ys: Vec<_> = boxes.iter().flat_map(|b| [b[1], b[3]]).collect();
    ys.sort_by(f64::total_cmp);
    ys.dedup();
    if ys.len() < 2 {
        return 0.0;
    }
    let mut events = Vec::with_capacity(boxes.len() * 2);
    for b in boxes {
        let start = ys.binary_search_by(|y| y.total_cmp(&b[1])).unwrap_or(0);
        let end = ys.binary_search_by(|y| y.total_cmp(&b[3])).unwrap_or(0);
        events.push((b[0], start, end, 1));
        events.push((b[2], start, end, -1));
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut tree = Coverage {
        counts: vec![0; ys.len() * 4],
        lengths: vec![0.0; ys.len() * 4],
        ys,
    };
    let mut previous = events.first().map_or(0.0, |e| e.0);
    let mut area = 0.0;
    for (x, start, end, delta) in events {
        area += (x - previous) * tree.lengths[1];
        previous = x;
        tree.update(1, (0, tree.ys.len() - 1), (start, end), delta);
    }
    area
}

struct Coverage {
    counts: Vec<i32>,
    lengths: Vec<f64>,
    ys: Vec<f64>,
}
impl Coverage {
    fn update(&mut self, node: usize, range: (usize, usize), interval: (usize, usize), delta: i32) {
        let (lo, hi) = range;
        if interval.0 >= hi || interval.1 <= lo {
            return;
        }
        if interval.0 <= lo && hi <= interval.1 {
            self.counts[node] += delta;
        } else {
            let mid = lo.midpoint(hi);
            self.update(node * 2, (lo, mid), interval, delta);
            self.update(node * 2 + 1, (mid, hi), interval, delta);
        }
        self.lengths[node] = if self.counts[node] > 0 {
            self.ys[hi] - self.ys[lo]
        } else if hi - lo == 1 {
            0.0
        } else {
            self.lengths[node * 2] + self.lengths[node * 2 + 1]
        };
    }
}
