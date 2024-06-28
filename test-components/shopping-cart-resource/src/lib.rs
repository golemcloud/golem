mod bindings;

use crate::bindings::exports::golem::it::api::*;
use std::cell::RefCell;

use rand::prelude::*;

struct Component;

impl Guest for Component {
    type Cart = crate::Cart;
}

pub struct Cart {
    user_id: String,
    items: RefCell<Vec<ProductItem>>,
}

impl GuestCart for Cart {
    fn new(user_id: String) -> Self {
        Self {
            user_id,
            items: RefCell::new(Vec::new()),
        }
    }

    fn add_item(&self, item: ProductItem) {
        println!(
            "Adding item {:?} to the cart of user {}",
            item, self.user_id
        );

        self.items.borrow_mut().push(item);
    }

    fn remove_item(&self, product_id: String) {
        println!(
            "Removing item with product ID {} from the cart of user {}",
            product_id, self.user_id
        );

        self.items
            .borrow_mut()
            .retain(|item| item.product_id != product_id);
    }

    fn update_item_quantity(&self, product_id: String, quantity: u32) {
        println!(
            "Updating quantity of item with product ID {} to {} in the cart of user {}",
            product_id, quantity, self.user_id
        );

        for item in &mut *self.items.borrow_mut() {
            if item.product_id == product_id {
                item.quantity = quantity;
            }
        }
    }

    fn checkout(&self) -> CheckoutResult {
        let result = do_checkout(self);

        match result {
            Ok(OrderConfirmation { order_id }) => {
                CheckoutResult::Success(OrderConfirmation { order_id })
            }
            Err(err) => CheckoutResult::Error(err.to_string()),
        }
    }

    fn get_cart_contents(&self) -> Vec<ProductItem> {
        println!("Getting cart contents for user {}", self.user_id);

        self.items.borrow().clone()
    }

    fn merge_with(&self, _other_cart: CartBorrow<'_>) {
        todo!()
    }
}

fn do_checkout(cart: &Cart) -> Result<OrderConfirmation, &'static str> {
    reserve_inventory()?;
    charge_credit_card()?;
    let order_id = generate_order();
    dispatch_order()?;
    cart.items.borrow_mut().clear();

    println!("Checkout for order {}", order_id);

    Ok(OrderConfirmation { order_id })
}

fn reserve_inventory() -> Result<(), &'static str> {
    // generate a random float 32:
    let mut rng = rand::thread_rng();
    let random_float: f32 = rng.gen();

    // Reserve inventory for the items in the cart.
    // If the inventory is not available, return an error.
    // Otherwise, return a success result.
    if random_float < 0.1 {
        return Err("Inventory not available");
    } else {
        Ok(())
    }
}

#[allow(unused)]
fn release_inventory() -> Result<(), &'static str> {
    // Release inventory for the items in the cart.
    // If the inventory is not available, return an error.
    // Otherwise, return a success result.
    Ok(())
}

fn charge_credit_card() -> Result<(), &'static str> {
    // Charge the user's credit card for the items in the cart.
    // If the charge fails, return an error.
    // Otherwise, return a success result.
    Ok(())
}

fn generate_order() -> String {
    // Save the order to the database.
    // Return the order ID.
    "238738674".to_string()
}

fn dispatch_order() -> Result<(), &'static str> {
    // Dispatch the order to the warehouse.
    // If the order cannot be dispatched, return an error.
    // Otherwise, return a success result.
    Ok(())
}

bindings::export!(Component with_types_in bindings);
