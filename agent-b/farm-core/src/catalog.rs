use crate::types::{Category, Product};

/// Returns the full hardcoded product catalog.
pub fn get_catalog() -> Vec<Product> {
    vec![
        Product {
            id: "farm-eggs-dozen".into(),
            name: "Farm Fresh Eggs (1 Dozen)".into(),
            description: "Free-range eggs from pasture-raised hens, collected daily.".into(),
            category: Category::Poultry,
            price_cents: 599,
            unit: "dozen".into(),
            in_stock: true,
        },
        Product {
            id: "farm-eggs-half".into(),
            name: "Farm Fresh Eggs (Half Dozen)".into(),
            description: "Free-range eggs, half dozen pack.".into(),
            category: Category::Poultry,
            price_cents: 349,
            unit: "half-dozen".into(),
            in_stock: true,
        },
        Product {
            id: "farm-beef-ground".into(),
            name: "Grass-Fed Ground Beef".into(),
            description: "100% grass-fed and grass-finished ground beef, 85/15 lean.".into(),
            category: Category::Meat,
            price_cents: 899,
            unit: "lb".into(),
            in_stock: true,
        },
        Product {
            id: "farm-beef-ribeye".into(),
            name: "Grass-Fed Ribeye Steak".into(),
            description: "Premium grass-fed ribeye, dry-aged 21 days.".into(),
            category: Category::Meat,
            price_cents: 1899,
            unit: "lb".into(),
            in_stock: true,
        },
        Product {
            id: "farm-beef-sirloin".into(),
            name: "Grass-Fed Sirloin Steak".into(),
            description: "Lean grass-fed sirloin, perfect for grilling.".into(),
            category: Category::Meat,
            price_cents: 1499,
            unit: "lb".into(),
            in_stock: true,
        },
        Product {
            id: "farm-milk-whole".into(),
            name: "Whole Milk".into(),
            description: "Fresh whole milk from grass-fed cows, non-homogenized.".into(),
            category: Category::Dairy,
            price_cents: 499,
            unit: "gallon".into(),
            in_stock: true,
        },
        Product {
            id: "farm-milk-half".into(),
            name: "Whole Milk (Half Gallon)".into(),
            description: "Fresh whole milk, half gallon size.".into(),
            category: Category::Dairy,
            price_cents: 299,
            unit: "half-gallon".into(),
            in_stock: true,
        },
        Product {
            id: "farm-butter".into(),
            name: "Farm Churned Butter".into(),
            description: "Traditional churned butter from grass-fed cream, lightly salted.".into(),
            category: Category::Dairy,
            price_cents: 549,
            unit: "lb".into(),
            in_stock: true,
        },
        Product {
            id: "farm-cheese-cheddar".into(),
            name: "Aged Cheddar Cheese".into(),
            description: "Sharp cheddar aged 12 months, made from raw milk.".into(),
            category: Category::Dairy,
            price_cents: 799,
            unit: "lb".into(),
            in_stock: true,
        },
        Product {
            id: "farm-chicken-breast".into(),
            name: "Free-Range Chicken Breast".into(),
            description: "Boneless skinless chicken breast from free-range birds.".into(),
            category: Category::Poultry,
            price_cents: 999,
            unit: "lb".into(),
            in_stock: true,
        },
    ]
}

/// Find a product by ID.
pub fn find_product(product_id: &str) -> Option<Product> {
    get_catalog().into_iter().find(|p| p.id == product_id)
}

/// List products, optionally filtered by category.
pub fn list_products(category: Option<&str>) -> Vec<Product> {
    let catalog = get_catalog();
    match category {
        Some(cat) => catalog
            .into_iter()
            .filter(|p| p.category.as_str() == cat)
            .collect(),
        None => catalog,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_has_10_products() {
        assert_eq!(get_catalog().len(), 10);
    }

    #[test]
    fn test_find_product_exists() {
        let p = find_product("farm-eggs-dozen");
        assert!(p.is_some());
        let p = p.unwrap();
        assert_eq!(p.name, "Farm Fresh Eggs (1 Dozen)");
        assert_eq!(p.price_cents, 599);
    }

    #[test]
    fn test_find_product_not_found() {
        assert!(find_product("nonexistent").is_none());
    }

    #[test]
    fn test_list_products_no_filter() {
        let products = list_products(None);
        assert_eq!(products.len(), 10);
    }

    #[test]
    fn test_list_products_by_dairy() {
        let products = list_products(Some("dairy"));
        assert_eq!(products.len(), 4); // milk-whole, milk-half, butter, cheese
        for p in &products {
            assert_eq!(p.category, Category::Dairy);
        }
    }

    #[test]
    fn test_list_products_by_meat() {
        let products = list_products(Some("meat"));
        assert_eq!(products.len(), 3); // ground, ribeye, sirloin
    }

    #[test]
    fn test_list_products_by_poultry() {
        let products = list_products(Some("poultry"));
        assert_eq!(products.len(), 3); // eggs dozen, eggs half, chicken
    }

    #[test]
    fn test_list_products_by_produce() {
        let products = list_products(Some("produce"));
        assert_eq!(products.len(), 0); // none in current catalog
    }

    #[test]
    fn test_all_products_in_stock() {
        for p in get_catalog() {
            assert!(p.in_stock, "Product {} should be in stock", p.id);
        }
    }

    #[test]
    fn test_all_products_have_positive_price() {
        for p in get_catalog() {
            assert!(p.price_cents > 0, "Product {} should have positive price", p.id);
        }
    }

    #[test]
    fn test_product_ids_unique() {
        let catalog = get_catalog();
        let mut ids: Vec<&str> = catalog.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), catalog.len());
    }
}
