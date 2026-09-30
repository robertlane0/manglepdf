//! Document permission bits.

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

/// The `/P` permission bits of the standard security handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions {
    bits: i32,
}

/// Bit positions in `/P`, counting from 1 at the *most* significant of the low 32 bits.
const BIT_PRINT: u32 = 3;
const BIT_MODIFY: u32 = 4;
const BIT_EXTRACT: u32 = 5;
const BIT_ANNOTATE: u32 = 6;
const BIT_FILL_FORMS: u32 = 9;
const BIT_EXTRACT_ACCESS: u32 = 10;
const BIT_ASSEMBLE: u32 = 11;
const BIT_FAITHFUL: u32 = 12;

impl Permissions {
    #[must_use]
    pub fn from_bits(bits: i32) -> Self {
        Self { bits }
    }

    #[must_use]
    pub fn bits(&self) -> i32 {
        self.bits
    }

    #[must_use]
    pub fn all(&self) -> bool {
        // Bits 1, 2 and 7-8 are reserved and must be 1 for the document to be valid.
        self.allows_everything()
    }

    fn allows_everything(&self) -> bool {
        self.bits & PERMISSION_MASK == PERMISSION_MASK
    }

    /// Printing is allowed.
    #[must_use]
    pub fn print(&self) -> bool {
        self.get(BIT_PRINT)
    }

    /// Modifying the document is allowed.
    #[must_use]
    pub fn modify(&self) -> bool {
        self.get(BIT_MODIFY)
    }

    /// Copying text and graphics is allowed.
    #[must_use]
    pub fn extract(&self) -> bool {
        self.get(BIT_EXTRACT) && self.get(BIT_EXTRACT_ACCESS)
    }

    /// Adding annotations and filling forms is allowed.
    #[must_use]
    pub fn annotate(&self) -> bool {
        self.get(BIT_ANNOTATE)
    }

    #[must_use]
    pub fn fill_forms(&self) -> bool {
        self.get(BIT_FILL_FORMS)
    }

    /// Inserting, rotating or deleting pages is allowed.
    #[must_use]
    pub fn assemble(&self) -> bool {
        self.get(BIT_ASSEMBLE)
    }

    #[must_use]
    pub fn faithful(&self) -> bool {
        self.get(BIT_FAITHFUL)
    }

    /// `/P` bit `bit` is counted from the *most* significant end, so bit 1 is 0x80000000.
    fn get(&self, bit: u32) -> bool {
        self.bits & mask(bit) != 0
    }

    /// Set or clear bit `bit` (counted from the most significant end).
    #[must_use]
    pub fn set(bit: u32, on: bool, bits: i32) -> i32 {
        if on {
            bits | mask(bit)
        } else {
            bits & !mask(bit)
        }
    }
}

/// Bits that must be set for the permissions to be well formed.
fn mask(bit: u32) -> i32 {
    1i32 << (32 - bit.min(32))
}

/// Bits 1, 2, 7 and 8 (counting from the most significant) are reserved and must be 1.
const RESERVED_MASK: i32 = 0xC300_0000_u32 as i32;
/// Every bit that is not reserved is a permission.
const PERMISSION_MASK: i32 = !RESERVED_MASK;

/// A short sentence for the UI's restriction banner.
#[must_use]
pub fn describe(p: &Permissions) -> String {
    if p.all() {
        return "no restrictions".to_string();
    }
    let mut denied = Vec::new();
    if !p.print() {
        denied.push("printing");
    }
    if !p.modify() {
        denied.push("editing");
    }
    if !p.extract() {
        denied.push("copying");
    }
    if !p.annotate() {
        denied.push("comments");
    }
    if !p.assemble() {
        denied.push("page assembly");
    }
    if !p.fill_forms() {
        denied.push("filling forms");
    }
    if denied.is_empty() {
        "no restrictions".to_string()
    } else {
        format!("restricted: no {}", denied.join(", "))
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn all_permissions() {
        let p = Permissions::from_bits(!RESERVED_MASK);
        assert!(p.all());
        assert!(p.print() && p.modify() && p.extract() && p.annotate() && p.assemble());
    }

    #[test]
    fn no_print_no_modify() {
        // The common "read only" pattern: everything off except the reserved bits.
        let p = Permissions::from_bits(RESERVED_MASK);
        assert!(!p.all());
        assert!(!p.print());
        assert!(!p.modify());
        assert!(!p.extract());
        assert!(!p.annotate());
        assert!(!p.fill_forms());
    }

    #[test]
    fn bit_helpers_agree() {
        let mut bits = RESERVED_MASK;
        for (bit, _) in [
            (BIT_PRINT, "print"),
            (BIT_MODIFY, "modify"),
            (BIT_EXTRACT, "extract"),
            (BIT_ANNOTATE, "annotate"),
            (BIT_FILL_FORMS, "fill"),
            (BIT_EXTRACT_ACCESS, "access"),
            (BIT_ASSEMBLE, "assemble"),
        ] {
            bits = Permissions::set(bit, true, bits);
        }
        let p = Permissions::from_bits(bits);
        assert!(p.print() && p.modify() && p.extract() && p.annotate() && p.fill_forms());
        assert!(p.assemble());
    }

    #[test]
    fn description_mentions_the_blocked_things() {
        let p = Permissions::from_bits(RESERVED_MASK);
        let d = describe(&p);
        assert!(d.contains("printing"), "{d}");
        assert!(d.contains("editing"), "{d}");
    }
}
