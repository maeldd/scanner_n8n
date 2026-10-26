//! Catalogue local des références.
//!
//! Le CSV est produit par `scripts/import-notion-csv.py` à partir de l'export
//! Notion. Il sert uniquement à afficher le nom et le stock connu dans
//! l'application ; Notion reste la source de vérité.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug)]
// Certains champs viennent de l'export Notion et sont conservés pour les
// évolutions (affichage détaillé, quantités par drone/rampe, etc.).
#[allow(dead_code)]
pub struct Reference {
    pub reference: String,
    pub name: String,
    pub category: String,
    pub kind: String,
    pub stock: i64,
    pub stock_en_cours: i64,
    pub low_stock: bool,
    pub description: String,
    pub qty_per_drone: i64,
    pub qty_per_rampe: i64,
}

#[derive(Deserialize)]
struct RawReference {
    #[serde(default)]
    reference: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    stock: Option<i64>,
    #[serde(default)]
    stock_en_cours: Option<i64>,
    #[serde(default)]
    low_stock: Option<bool>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    qty_per_drone: Option<i64>,
    #[serde(default)]
    qty_per_rampe: Option<i64>,
}

pub struct Catalog {
    refs: HashMap<String, Reference>,
}

impl Catalog {
    pub fn empty() -> Self {
        Self {
            refs: HashMap::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let mut reader = csv::ReaderBuilder::new()
            .flexible(true)
            .trim(csv::Trim::All)
            .from_path(path)
            .map_err(|e| format!("Impossible d'ouvrir {} : {e}", path.display()))?;

        let mut refs = HashMap::new();
        let mut ignored = 0usize;

        for row in reader.deserialize::<RawReference>() {
            let row = match row {
                Ok(row) => row,
                Err(error) => {
                    ignored += 1;
                    eprintln!("Ligne de catalogue ignorée : {error}");
                    continue;
                }
            };

            let reference = row.reference.trim().to_ascii_uppercase();
            if reference.is_empty() {
                ignored += 1;
                continue;
            }

            refs.insert(
                reference.clone(),
                Reference {
                    reference,
                    name: row.name.trim().to_string(),
                    category: row.category.trim().to_string(),
                    kind: row.kind.trim().to_string(),
                    stock: row.stock.unwrap_or(0),
                    stock_en_cours: row.stock_en_cours.unwrap_or(0),
                    low_stock: row.low_stock.unwrap_or(false),
                    description: row.description.trim().to_string(),
                    qty_per_drone: row.qty_per_drone.unwrap_or(0),
                    qty_per_rampe: row.qty_per_rampe.unwrap_or(0),
                },
            );
        }

        if refs.is_empty() {
            return Err(format!(
                "Aucune référence lue dans {} ({} ligne(s) ignorée(s)).",
                path.display(),
                ignored
            ));
        }

        Ok(Self { refs })
    }

    pub fn len(&self) -> usize {
        self.refs.len()
    }

    pub fn lookup(&self, sku: &str) -> Option<&Reference> {
        self.refs.get(&sku.trim().to_ascii_uppercase())
    }

    pub fn update_stock(&mut self, sku: &str, stock: i64) {
        if let Some(reference) = self.refs.get_mut(&sku.trim().to_ascii_uppercase()) {
            reference.stock = stock;
        }
    }

    pub fn adjust_stock(&mut self, sku: &str, delta: i64) {
        if let Some(reference) = self.refs.get_mut(&sku.trim().to_ascii_uppercase()) {
            reference.stock = reference.stock.saturating_add(delta);
        }
    }

    pub fn low_stock_count(&self) -> usize {
        self.refs.values().filter(|r| r.low_stock).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn loads_clean_catalog_and_normalizes_references() {
        let path =
            std::env::temp_dir().join(format!("scanner_catalog_test_{}.csv", std::process::id()));
        {
            let mut file = std::fs::File::create(&path).unwrap();
            writeln!(
                file,
                "reference,name,category,kind,stock,stock_en_cours,low_stock,description,qty_per_drone,qty_per_rampe"
            )
            .unwrap();
            writeln!(
                file,
                "dco001,Loctite,Consommable,Store,11,0,true,Étanchéité,1,0"
            )
            .unwrap();
        }

        let catalog = Catalog::load(&path).unwrap();
        assert_eq!(catalog.len(), 1);
        let reference = catalog.lookup("dco001").unwrap();
        assert_eq!(reference.reference, "DCO001");
        assert_eq!(reference.name, "Loctite");
        assert_eq!(reference.stock, 11);
        assert!(reference.low_stock);

        let _ = std::fs::remove_file(path);
    }
}
