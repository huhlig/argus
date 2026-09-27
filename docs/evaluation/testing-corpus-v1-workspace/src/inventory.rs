//! Seed `public-api-without-integration-test`: the public inventory API is only unit tested
//! piecemeal; nothing exercises it from outside the crate.

use std::collections::BTreeMap;

/// Stock levels by item name.
#[derive(Debug, Default)]
pub struct Inventory {
    stock: BTreeMap<String, u32>,
}

impl Inventory {
    /// Adds stock for an item.
    pub fn add(&mut self, item: &str, quantity: u32) {
        *self.stock.entry(item.to_owned()).or_default() += quantity;
    }

    /// Removes stock, failing when not enough is available.
    pub fn remove(&mut self, item: &str, quantity: u32) -> Result<(), String> {
        let available = self.stock.get_mut(item).ok_or_else(|| format!("unknown item {item}"))?;
        if *available < quantity {
            return Err(format!("only {available} {item} available"));
        }
        *available -= quantity;
        Ok(())
    }

    /// Total units across all items.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.stock.values().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_stock() {
        let mut inventory = Inventory::default();
        inventory.add("bolt", 3);
        assert_eq!(inventory.total(), 3);
    }
}
