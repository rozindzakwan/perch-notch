//! Mirroring other apps' notifications into the notch: which ones are new.
//!
//! The platform layer reads the notification list (Windows keeps it in the
//! action center) and hands the ids here. The first snapshot only records
//! what is already there, so starting Perch or turning the feature on does
//! not replay old notifications. Pure.

use std::collections::HashSet;

/// Ids seen so far.
#[derive(Debug, Clone, Default)]
pub struct SeenNotifications {
    seen: HashSet<u32>,
    primed: bool,
}

impl SeenNotifications {
    /// Feed the ids currently listed; returns those not seen before, in the
    /// order given. Empty on the first call. Ids no longer listed are
    /// forgotten, so memory stays bounded by the list.
    pub fn update(&mut self, current: &[u32]) -> Vec<u32> {
        let new: Vec<u32> = if self.primed {
            current
                .iter()
                .copied()
                .filter(|id| !self.seen.contains(id))
                .collect()
        } else {
            Vec::new()
        };
        self.primed = true;
        self.seen = current.iter().copied().collect();
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_snapshot_is_quiet_and_later_ones_report_only_new_ids() {
        let mut s = SeenNotifications::default();
        assert!(
            s.update(&[1, 2, 3]).is_empty(),
            "existing ones are not replayed"
        );
        assert!(s.update(&[1, 2, 3]).is_empty());
        assert_eq!(s.update(&[1, 2, 3, 7, 5]), vec![7, 5]);
        // Dismissed ones drop out; a re-used id later counts as new again.
        assert!(s.update(&[5]).is_empty());
        assert_eq!(s.update(&[5, 1]), vec![1]);
    }
}
