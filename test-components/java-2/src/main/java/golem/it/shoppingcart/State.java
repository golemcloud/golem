package golem.it.shoppingcart;


import wit.worlds.ShoppingCart;

import java.util.ArrayList;
import java.util.LinkedList;
import java.util.List;
import java.util.function.Function;
import java.util.stream.Collectors;

public final class State {
    private String userId;
    private List<ShoppingCart.ProductItem> items;

    public State() {
        this.userId = "";
        this.items = new LinkedList<>();
    }

    public String getUserId() {
        return userId;
    }

    public void setUserId(String userId) {
        this.userId = userId;
    }

    public void addItem(ShoppingCart.ProductItem item) {
        items.add(item);
    }

    public void removeItem(String productId) {
        items.removeIf(item -> item.productId.equals(productId));
    }

    public void updateItem(String productId, Function<ShoppingCart.ProductItem, ShoppingCart.ProductItem> f) {
        items = items.stream().map(item -> item.productId.equals(productId) ? f.apply(item) : item).collect(Collectors.toList());
    }

    public void clear() {
        items.clear();
    }

    public ArrayList<ShoppingCart.ProductItem> getItems() {
        return new ArrayList<>(items);
    }
}
