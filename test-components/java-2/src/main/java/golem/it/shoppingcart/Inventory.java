package golem.it.shoppingcart;

import java.util.Random;

public class Inventory {
    private static final Inventory instance = new Inventory();
    public static Inventory getInstance() {
        return instance;
    }

    private final Random random;

    private Inventory() {
        random = new Random();
    }

    public void reserve() {
        float randomFloat = random.nextFloat();
        if (randomFloat < 0.5) {
            throw new RuntimeException("golem.it.shoppingcart.Inventory is not available");
        }
    }
}
