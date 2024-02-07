use wit_value::*;

fn main() {
    let wit_value = WitValue::builder()
        .record()
        .field("a").u8(1)
        .field("b").record()
        .field("x").u8(2)
        .field("y").f64(0.1)
        .field("variant").variant("case1")
        .record()
        .field("case1_a").string("xyz")
        .field("case1_b").bool(true)
        .finish()
        .finish()
        .finish()
        .field("description").string("A record with a nested record.")
        .field("coordinates").tuple()
        .item().f64(1.0).item().f64(2.0).item().f64(3.0).finish()
        .field("tags").list().item().string("tag1").item().string("tag2").finish()
        .finish();
    println!("wit_value: {:?}", wit_value);
}
