use crate::catalog::find_product;
use crate::types::{Cart, CartItem};

/// Add a product to the cart. If product already in cart, increment quantity.
/// Returns Ok(updated cart) or Err(error message).
pub fn add_to_cart(cart: &mut Cart, product_id: &str, quantity: u32) -> Result<(), String> {
    if quantity == 0 {
        return Err("Quantity must be at least 1".into());
    }

    let product = find_product(product_id)
        .ok_or_else(|| format!("Product '{}' not found", product_id))?;

    if !product.in_stock {
        return Err(format!("Product '{}' is out of stock", product_id));
    }

    // Check if product is already in cart — increment quantity
    if let Some(item) = cart.items.iter_mut().find(|i| i.product_id == product_id) {
        item.quantity += quantity;
    } else {
        cart.items.push(CartItem {
            product_id: product_id.to_string(),
            quantity,
            unit_price_cents: product.price_cents,
        });
    }

    cart.recalculate_total();
    Ok(())
}

/// Remove a product entirely from the cart.
pub fn remove_from_cart(cart: &mut Cart, product_id: &str) -> Result<(), String> {
    let before = cart.items.len();
    cart.items.retain(|i| i.product_id != product_id);
    if cart.items.len() == before {
        return Err(format!("Product '{}' not in cart", product_id));
    }
    cart.recalculate_total();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Cart;

    fn new_cart() -> Cart {
        Cart::new("test-session".into())
    }

    #[test]
    fn test_add_single_item() {
        let mut cart = new_cart();
        add_to_cart(&mut cart, "farm-eggs-dozen", 1).unwrap();
        assert_eq!(cart.items.len(), 1);
        assert_eq!(cart.items[0].quantity, 1);
        assert_eq!(cart.total_cents, 599);
    }

    #[test]
    fn test_add_multiple_quantity() {
        let mut cart = new_cart();
        add_to_cart(&mut cart, "farm-eggs-dozen", 3).unwrap();
        assert_eq!(cart.items[0].quantity, 3);
        assert_eq!(cart.total_cents, 599 * 3);
    }

    #[test]
    fn test_add_increments_existing_item() {
        let mut cart = new_cart();
        add_to_cart(&mut cart, "farm-eggs-dozen", 1).unwrap();
        add_to_cart(&mut cart, "farm-eggs-dozen", 2).unwrap();
        assert_eq!(cart.items.len(), 1);
        assert_eq!(cart.items[0].quantity, 3);
        assert_eq!(cart.total_cents, 599 * 3);
    }

    #[test]
    fn test_add_multiple_products() {
        let mut cart = new_cart();
        add_to_cart(&mut cart, "farm-eggs-dozen", 1).unwrap();
        add_to_cart(&mut cart, "farm-milk-whole", 2).unwrap();
        assert_eq!(cart.items.len(), 2);
        assert_eq!(cart.total_cents, 599 + 499 * 2);
    }

    #[test]
    fn test_add_invalid_product() {
        let mut cart = new_cart();
        let result = add_to_cart(&mut cart, "nonexistent", 1);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_add_zero_quantity() {
        let mut cart = new_cart();
        let result = add_to_cart(&mut cart, "farm-eggs-dozen", 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least 1"));
    }

    #[test]
    fn test_remove_item() {
        let mut cart = new_cart();
        add_to_cart(&mut cart, "farm-eggs-dozen", 2).unwrap();
        add_to_cart(&mut cart, "farm-milk-whole", 1).unwrap();
        remove_from_cart(&mut cart, "farm-eggs-dozen").unwrap();
        assert_eq!(cart.items.len(), 1);
        assert_eq!(cart.items[0].product_id, "farm-milk-whole");
        assert_eq!(cart.total_cents, 499);
    }

    #[test]
    fn test_remove_nonexistent_item() {
        let mut cart = new_cart();
        let result = remove_from_cart(&mut cart, "farm-eggs-dozen");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in cart"));
    }

    #[test]
    fn test_cart_total_complex() {
        let mut cart = new_cart();
        add_to_cart(&mut cart, "farm-beef-ribeye", 2).unwrap();  // 1899 * 2 = 3798
        add_to_cart(&mut cart, "farm-butter", 1).unwrap();       // 549
        add_to_cart(&mut cart, "farm-eggs-dozen", 3).unwrap();   // 599 * 3 = 1797
        assert_eq!(cart.total_cents, 3798 + 549 + 1797);        // = 6144
    }

    #[test]
    fn test_empty_cart_total() {
        let cart = new_cart();
        assert_eq!(cart.total_cents, 0);
        assert!(cart.items.is_empty());
    }
}
