//! Rows that grow to fit their text: free (AutoCAD's behaviour), in whole steps (ESKD forms,
//! rows of 8 mm) or never. A row grows from the height it was given and shrinks back to it.

use serde::{Deserialize, Serialize};

use crate::Table;

/// How a table's rows grow when their text needs more height than they were given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowGrow {
    /// As much as the text needs.
    #[default]
    Free,
    /// To a whole multiple of the step (8: rows of 8, 16, 24 …).
    Step(f64),
    /// Never: rows keep their heights and the text runs over.
    Fixed,
}

impl RowGrow {
    /// A step that is not a finite size more than 0 becomes free growth.
    pub fn sanitized(self) -> RowGrow {
        match self {
            RowGrow::Step(s) if !(s.is_finite() && s > 0.0 && s <= 1e9) => RowGrow::Free,
            g => g,
        }
    }
    pub fn is_free(&self) -> bool {
        *self == RowGrow::Free
    }
    /// The height of a row given `base` whose content needs `need`.
    fn grow(self, base: f64, need: f64) -> f64 {
        if !need.is_finite() || need <= base + 1e-9 {
            return base;
        }
        match self.sanitized() {
            RowGrow::Free => need,
            RowGrow::Fixed => base,
            RowGrow::Step(s) => {
                let n = (need / s - 1e-9).ceil().min(1e6);
                (n * s).max(base)
            }
        }
    }
}

impl Table {
    /// The height row `r` was given (by TABLESIZE or when made), before growing for its text.
    pub fn row_base(&self, r: usize) -> f64 {
        let cur = self.row_heights.get(r).copied().unwrap_or(0.0);
        if self.row_min.len() != self.row_heights.len() {
            return cur;
        }
        self.row_min.get(r).copied().filter(|h| h.is_finite() && *h > 0.0).unwrap_or(cur)
    }

    /// Give row `r` height `h`: the height it grows from.
    pub fn set_row_height(&mut self, r: usize, h: f64) {
        if self.row_min.len() != self.row_heights.len() {
            self.row_min = self.row_heights.clone();
        }
        if let Some(slot) = self.row_heights.get_mut(r) {
            *slot = h;
        }
        if let Some(slot) = self.row_min.get_mut(r) {
            *slot = h;
        }
    }

    /// Size the rows to their content: `needs[r]` is the height row `r`'s own cells need,
    /// `spans` (first row, rows, height) what cells merged over several rows need (the last
    /// row of the merge takes what is missing). Rows grow from their given heights by
    /// [`Table::row_grow`] and shrink back to them when the content needs less.
    pub fn fit_rows(&mut self, needs: &[f64], spans: &[(usize, usize, f64)]) {
        let n = self.row_heights.len();
        let base: Vec<f64> = (0..n).map(|r| self.row_base(r)).collect();
        let grow = self.row_grow.sanitized();
        let mut h: Vec<f64> = base.iter().enumerate().map(|(r, b)| grow.grow(*b, needs.get(r).copied().unwrap_or(0.0))).collect();
        for &(r0, rows, need) in spans {
            let Some(last) = r0.checked_add(rows.max(1)).map(|e| e.min(n)).filter(|e| *e > r0).map(|e| e - 1) else { continue };
            let others: f64 = h.get(r0..last).map_or(0.0, |s| s.iter().sum());
            if let (Some(slot), Some(b)) = (h.get_mut(last), base.get(last)) {
                *slot = grow.grow((*slot).max(*b), need - others);
            }
        }
        let changed = h.iter().zip(&base).any(|(a, b)| (a - b).abs() > 1e-9);
        self.row_heights = h;
        self.row_min = if changed { base } else { Vec::new() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(heights: &[f64], grow: RowGrow) -> Table {
        Table { row_heights: heights.to_vec(), col_widths: vec![10.0], row_grow: grow, ..Table::default() }
    }

    #[test]
    fn rows_grow_free_in_steps_or_not_at_all_and_shrink_back() {
        let mut t = table(&[8.0, 8.0], RowGrow::Free);
        t.fit_rows(&[9.5, 3.0], &[]);
        assert_eq!(t.row_heights, vec![9.5, 8.0]);
        assert_eq!(t.row_min, vec![8.0, 8.0]);
        // The text got shorter: back to the given height.
        t.fit_rows(&[3.0, 3.0], &[]);
        assert_eq!(t.row_heights, vec![8.0, 8.0]);
        assert!(t.row_min.is_empty());

        let mut t = table(&[8.0, 15.0], RowGrow::Step(8.0));
        t.fit_rows(&[9.5, 16.5], &[]);
        assert_eq!(t.row_heights, vec![16.0, 24.0]);
        t.fit_rows(&[16.0, 0.0], &[]);
        assert_eq!(t.row_heights, vec![16.0, 15.0]);

        let mut t = table(&[8.0], RowGrow::Fixed);
        t.fit_rows(&[30.0], &[]);
        assert_eq!(t.row_heights, vec![8.0]);
    }

    #[test]
    fn a_merge_over_rows_grows_its_last_row() {
        let mut t = table(&[8.0, 8.0, 8.0], RowGrow::Step(8.0));
        t.fit_rows(&[0.0; 3], &[(0, 2, 20.0)]);
        assert_eq!(t.row_heights, vec![8.0, 16.0, 8.0]);
        // Hostile spans and steps change nothing and never panic.
        let mut t = table(&[8.0], RowGrow::Step(f64::NAN));
        t.fit_rows(&[f64::INFINITY], &[(5, 3, 1.0), (0, usize::MAX, f64::NAN), (0, 0, 9.0)]);
        assert_eq!(t.row_heights, vec![9.0]);
    }

    #[test]
    fn set_row_height_is_what_the_row_grows_from() {
        let mut t = table(&[8.0, 8.0], RowGrow::Free);
        t.fit_rows(&[12.0, 0.0], &[]);
        t.set_row_height(1, 10.0);
        assert_eq!(t.row_base(0), 8.0);
        assert_eq!(t.row_base(1), 10.0);
        t.fit_rows(&[0.0, 0.0], &[]);
        assert_eq!(t.row_heights, vec![8.0, 10.0]);
    }
}
