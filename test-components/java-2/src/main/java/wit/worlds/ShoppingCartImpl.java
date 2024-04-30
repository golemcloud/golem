package wit.worlds;

import golem.it.shoppingcart.Inventory;
import golem.it.shoppingcart.State;

import java.util.ArrayList;

public final class ShoppingCartImpl {

    private static final State state = new State();

    public static void initializeCart(String userId) {
        System.out.println("Initializing the cart for user " + userId);
        state.setUserId(userId);
    }

    public static void addItem(ShoppingCart.ProductItem item) {
        System.out.println("Adding item " + item.productId + " to the cart of user " + state.getUserId());
        state.addItem(item);
    }

    public static void removeItem(String productId) {
        System.out.println("Removing item " + productId + " from the cart of user " + state.getUserId());
        state.removeItem(productId);
    }

    public static void updateItemQuantity(String productId, int quantity) {
        System.out.println("Updating quantity of item " + productId + " to " + quantity + " in the cart of user " + state.getUserId());
        state.updateItem(productId, (item) -> new ShoppingCart.ProductItem(item.productId, item.name, item.price, quantity));
    }

    public static ShoppingCart.CheckoutResult checkout() {
        Inventory.getInstance().reserve();

        String orderId = "1234";
        System.out.println("Checkout for order " + orderId);
        state.clear();
        return ShoppingCart.CheckoutResult.success(new ShoppingCart.OrderConfirmation(orderId));
    }

    public static ArrayList<ShoppingCart.ProductItem> getCartContents() {
        System.out.println("Getting cart contents for user " + state.getUserId());
        return state.getItems();
    }

}

