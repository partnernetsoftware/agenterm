//! Numeric dotted versions (`0.2.3`, `1.0`). Pre-release tags are refused,
//! not guessed: MiniCon releases carry plain numeric versions.

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(pub Vec<u64>);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Option<Vec<u64>> = s.split('.').map(|p| p.parse().ok()).collect();
        let parts = parts?;
        if parts.is_empty() { None } else { Some(Version(parts)) }
    }

    pub fn major(&self) -> u64 {
        self.0[0]
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// Missing trailing components count as zero, so `1.0 == 1.0.0`.
    fn cmp(&self, other: &Self) -> Ordering {
        let n = self.0.len().max(other.0.len());
        (0..n)
            .map(|i| self.0.get(i).copied().unwrap_or(0).cmp(&other.0.get(i).copied().unwrap_or(0)))
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal)
    }
}
