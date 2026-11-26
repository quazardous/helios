//! The camera a source's photos come from, as its connector declares it
//! ([`crate::SourceRef::camera`]): a card's or a camera's make and model,
//! read by the connector (from the first files' EXIF, from the camera
//! itself). The host shows what is declared: the camera's name, and a
//! drawing of its body's shape ([`Camera::kind`]), chosen here from the
//! make and model by a small table. No picture of a real product ships:
//! the drawings are generic, one per kind.

use serde::{Deserialize, Serialize};

/// A camera, by its make and model as its files name them (EXIF `Make`
/// and `Model`: `NIKON CORPORATION`, `NIKON Z 50`) or as a connector
/// says them (`Nikon`, `Z 50`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Camera {
    pub make: String,
    pub model: String,
}

/// A camera body's shape: what its drawing shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CameraKind {
    /// Interchangeable lenses, no mirror: a flat body, a short grip.
    Mirrorless,
    /// A mirror and a prism: the hump over the lens.
    Dslr,
    /// A fixed lens in a small box.
    Compact,
    /// A phone.
    Phone,
}

impl CameraKind {
    /// Its id, in kebab case (`mirrorless`, `dslr`): the drawing's name.
    pub fn id(self) -> &'static str {
        match self {
            CameraKind::Mirrorless => "mirrorless",
            CameraKind::Dslr => "dslr",
            CameraKind::Compact => "compact",
            CameraKind::Phone => "phone",
        }
    }
}

/// The makes as their files write them, and their short name.
const BRANDS: &[(&str, &str)] = &[
    ("nikon", "Nikon"),
    ("canon", "Canon"),
    ("sony", "Sony"),
    ("fujifilm", "Fujifilm"),
    ("panasonic", "Panasonic"),
    ("olympus", "Olympus"),
    ("om digital", "OM System"),
    ("pentax", "Pentax"),
    ("ricoh", "Ricoh"),
    ("leica", "Leica"),
    ("hasselblad", "Hasselblad"),
    ("sigma", "Sigma"),
    ("apple", "Apple"),
    ("google", "Google"),
    ("samsung", "Samsung"),
    ("xiaomi", "Xiaomi"),
    ("huawei", "Huawei"),
    ("oneplus", "OnePlus"),
    ("oppo", "OPPO"),
    ("motorola", "Motorola"),
    ("fairphone", "Fairphone"),
    ("gopro", "GoPro"),
    ("dji", "DJI"),
];

/// The makes whose cameras are phones.
const PHONES: &[&str] = &[
    "Apple",
    "Google",
    "Xiaomi",
    "Huawei",
    "OnePlus",
    "OPPO",
    "Motorola",
    "Fairphone",
];

impl Camera {
    pub fn new(make: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            make: make.into(),
            model: model.into(),
        }
    }

    /// Its make's short name (`NIKON CORPORATION` → `Nikon`), else its
    /// make as written.
    pub fn brand(&self) -> String {
        let make = self.make.trim().to_lowercase();
        BRANDS
            .iter()
            .find(|(k, _)| make.starts_with(k))
            .map_or_else(|| self.make.trim().to_owned(), |(_, b)| (*b).to_owned())
    }

    /// Its model without its make's name in front (`NIKON Z 50` → `Z 50`).
    fn bare_model(&self) -> &str {
        let model = self.model.trim();
        let brand = self.brand();
        let first = self.make.split_whitespace().next().unwrap_or_default();
        for prefix in [brand.as_str(), first] {
            if !prefix.is_empty()
                && model.len() > prefix.len()
                && model.is_char_boundary(prefix.len())
                && model[..prefix.len()].eq_ignore_ascii_case(prefix)
                && model[prefix.len()..].starts_with(' ')
            {
                return model[prefix.len()..].trim_start();
            }
        }
        model
    }

    /// Its name as shown: its make's short name and its model, once
    /// (`Nikon Z 50`).
    pub fn name(&self) -> String {
        let brand = self.brand();
        let model = self.bare_model();
        match (brand.is_empty(), model.is_empty()) {
            (true, _) => model.to_owned(),
            (false, true) => brand,
            (false, false) => format!("{brand} {model}"),
        }
    }

    /// Its body's shape, from its make and model; `None` when the table
    /// does not know it (the host keeps its generic card icon).
    pub fn kind(&self) -> Option<CameraKind> {
        use CameraKind::{Compact, Dslr, Mirrorless, Phone};
        let brand = self.brand();
        let model = self.bare_model().to_uppercase();
        let starts = |p: &str| model.starts_with(p);
        if PHONES.contains(&brand.as_str()) {
            return Some(Phone);
        }
        match brand.as_str() {
            "Samsung" if starts("SM-") || starts("GALAXY") => Some(Phone),
            "Samsung" if starts("NX") => Some(Mirrorless),
            "Nikon" if starts("Z") => Some(Mirrorless),
            "Nikon" if starts("1 ") => Some(Mirrorless),
            "Nikon" if starts("D") && model[1..].starts_with(|c: char| c.is_ascii_digit()) => {
                Some(Dslr)
            }
            "Nikon" if starts("COOLPIX") => Some(Compact),
            "Canon" if starts("EOS R") || starts("EOS M") => Some(Mirrorless),
            "Canon" if starts("EOS") => Some(Dslr),
            "Canon" if starts("POWERSHOT") || starts("IXUS") || starts("IXY") => Some(Compact),
            "Sony" if starts("ILCE") || starts("NEX") || starts("ZV-E") => Some(Mirrorless),
            "Sony" if starts("ILCA") || starts("SLT") || starts("DSLR") => Some(Dslr),
            "Sony" if starts("DSC") || starts("ZV-1") => Some(Compact),
            "Fujifilm" if starts("X100") || starts("XF") || starts("X30") || starts("XQ") => {
                Some(Compact)
            }
            "Fujifilm" if starts("X-") || starts("GFX") => Some(Mirrorless),
            "Panasonic"
                if ["DC-G", "DC-S", "DMC-G", "DC-BGH"]
                    .iter()
                    .any(|p| starts(p)) =>
            {
                Some(Mirrorless)
            }
            "Panasonic" if ["DC-", "DMC-"].iter().any(|p| starts(p)) => Some(Compact),
            "Olympus" | "OM System" if starts("E-M") || starts("E-P") || starts("OM-") => {
                Some(Mirrorless)
            }
            "Olympus" | "OM System" if starts("TG-") => Some(Compact),
            "Olympus" if starts("E-") => Some(Dslr),
            "Pentax" | "Ricoh" if starts("K") || starts("PENTAX K") => Some(Dslr),
            "Ricoh" | "Pentax" if starts("GR") => Some(Compact),
            "Leica" if starts("Q") || starts("D-LUX") || starts("C-LUX") => Some(Compact),
            "Leica" if starts("SL") || starts("CL") || starts("M") || starts("TL") => {
                Some(Mirrorless)
            }
            "Hasselblad" if starts("X") => Some(Mirrorless),
            "Sigma" if starts("FP") => Some(Mirrorless),
            "GoPro" | "DJI" => Some(Compact),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_is_named_by_its_brand_and_model_once() {
        assert_eq!(Camera::new("Nikon", "Z 50").name(), "Nikon Z 50");
        assert_eq!(
            Camera::new("NIKON CORPORATION", "NIKON Z 50").name(),
            "Nikon Z 50"
        );
        assert_eq!(Camera::new("Canon", "Canon EOS R6").name(), "Canon EOS R6");
        assert_eq!(Camera::new("Apple", "iPhone 15").name(), "Apple iPhone 15");
        assert_eq!(Camera::new("Acme", "Box 1").name(), "Acme Box 1");
    }

    #[test]
    fn a_camera_s_body_shape_comes_from_its_make_and_model() {
        let kind = |make: &str, model: &str| Camera::new(make, model).kind();
        assert_eq!(kind("Nikon", "Z 50"), Some(CameraKind::Mirrorless));
        assert_eq!(
            kind("NIKON CORPORATION", "NIKON D750"),
            Some(CameraKind::Dslr)
        );
        assert_eq!(kind("NIKON", "COOLPIX P1000"), Some(CameraKind::Compact));
        assert_eq!(kind("Canon", "Canon EOS R5"), Some(CameraKind::Mirrorless));
        assert_eq!(
            kind("Canon", "Canon EOS 5D Mark IV"),
            Some(CameraKind::Dslr)
        );
        assert_eq!(
            kind("Canon", "Canon PowerShot G7 X"),
            Some(CameraKind::Compact)
        );
        assert_eq!(kind("SONY", "ILCE-7M4"), Some(CameraKind::Mirrorless));
        assert_eq!(kind("FUJIFILM", "X100V"), Some(CameraKind::Compact));
        assert_eq!(kind("FUJIFILM", "X-T5"), Some(CameraKind::Mirrorless));
        assert_eq!(kind("Apple", "iPhone 15 Pro"), Some(CameraKind::Phone));
        assert_eq!(kind("samsung", "SM-S918B"), Some(CameraKind::Phone));
        // Unknown: the host keeps its card icon.
        assert_eq!(kind("Acme", "Box 1"), None);
        assert_eq!(kind("Nikon", ""), None);
    }
}
