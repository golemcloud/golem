mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::golem::api::host::*;

use rand::prelude::*;

struct Component;

/**
 * This is one of any number of data types that our application
 * uses. Golem will take care to persist all application state,
 * whether that state is local to a function being executed or
 * global across the entire program.
 */
struct State {
    user_id: String,
    items: Vec<ProductItem>,
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

/**
 * This holds the state of our application, which is always bound to
 * a given user. It is a global variable, which Rust doesn't like, so
 * we use `with_state` to access or update the global variable, so we
 * can avoid `unsafe` noise.
 */
static mut STATE: State = State {
    user_id: String::new(),
    items: vec![],
};

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let result = unsafe { f(&mut STATE) };

    return result;
}

// Here, we declare a Rust implementation of the `ShoppingCart` trait.
impl Guest for Component {
    fn initialize_cart(user_id: String) -> () {
        with_state(|state| {
            println!("Initializing cart for user {}", user_id);

            state.user_id = user_id;
        });
    }

    fn add_item(item: ProductItem) -> () {
        with_state(|state| {
            println!(
                "Adding item {:?} to the cart of user {}",
                item, state.user_id
            );

            state.items.push(item);
        });
    }

    fn remove_item(product_id: String) -> () {
        with_state(|state| {
            println!(
                "Removing item with product ID {} from the cart of user {}",
                product_id, state.user_id
            );

            state.items.retain(|item| item.product_id != product_id);
        });
    }

    fn update_item_quantity(product_id: String, quantity: u32) -> () {
        with_state(|state| {
            println!(
                "Updating quantity of item with product ID {} to {} in the cart of user {}",
                product_id, quantity, state.user_id
            );

            for item in &mut state.items {
                if item.product_id == product_id {
                    item.quantity = quantity;
                }
            }
        });
    }

    fn checkout() -> CheckoutResult {
        let result: Result<OrderConfirmation, &'static str> = with_state(|state| {
            reserve_inventory()?;

            charge_credit_card()?;

            let order_id = generate_order();

            dispatch_order()?;

            state.items.clear();

            println!("Checkout for order {}", order_id);

            Ok(OrderConfirmation { order_id })
        });

        match result {
            Ok(OrderConfirmation { order_id }) => {
                CheckoutResult::Success(OrderConfirmation { order_id })
            }
            Err(err) => CheckoutResult::Error(err.to_string()),
        }
    }

    fn get_cart_contents() -> Vec<ProductItem> {
        with_state(|state| {
            println!("Getting cart contents for user {}", state.user_id);

            state.items.clone()
        })
    }
}

bindings::export!(Component with_types_in bindings);
