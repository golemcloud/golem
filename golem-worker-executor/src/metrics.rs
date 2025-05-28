pub mod resources {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref FUEL_BORROW_TOTAL: Counter =
            register_counter!("fuel_borrow_total", "Total amount of fuel borrowed").unwrap();
        static ref FUEL_RETURN_TOTAL: Counter =
            register_counter!("fuel_return_total", "Total amount of fuel returned").unwrap();
    }

    pub fn record_fuel_borrow(amount: i64) {
        FUEL_BORROW_TOTAL.inc_by(amount as f64);
    }

    pub fn record_fuel_return(amount: i64) {
        FUEL_RETURN_TOTAL.inc_by(amount as f64);
    }
}
