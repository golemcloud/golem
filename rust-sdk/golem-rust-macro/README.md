# Golem Rust

This crate contains couple of Rust macros that facilitate writing Golem Cloud backends in Rust:
1. Derives `From<>` and `Into<>` typeclasses between wit-bindgen derived data types and custom domain model data types.
2. Generates wit file from rust code.

## Add to your project

```shell
$ cargo add golem-rust
```

## 1. Convert between generated data types and custom domain model

When working with WIT files in Golem, wit-bindgen library generates data types based on the wit file. There are few drawbacks when using these data types, so very often, user would create its own data types. In order to easily convert between generated and domain data types, programmer needs to implement boilerplate-y `From<>` and `Into<>` typeclasses.

This project contains macro that would automatically implement those typeclasses. 

### Struct

Let's say we have 
```rust
    pub struct Person {
        pub name: String,
        pub age: i32,
    }

    pub struct WitPerson {
        pub name: String,
        pub age: i32,
    }

```

We can use macro help in implementing `From` and `Into` typeclasses by annotating Person with `#[derive(golem_rust::WIT_From_Into))]` 

```rust
    #[derive(golem_rust::WIT_From_Into))]
    pub struct Person {
        pub name: String,
        pub age: i32,
    }
```

then the following code compiles without problems

```rust 
    let me = Person {
        name: "Jaro".to_owned(),
        age: 32,
    };

    let converted: WitPerson = me.into();
```

#### Custom data type names

The above macro assumed that the data type for which we are deriving `From<>` and `Into<>` is called `WitPerson`. By default macro assumes that the name of the data type is `Wit` + annotated data type name. In case the name is different, we need to add `#[wit_type_name(DerivedName)]` attribute. 

```rust
    #[derive(golem_rust::WIT_From_Into))]
    #[wit_type_name(DerivedName)]
    pub struct Person {
        pub name: String,
        pub age: i32,
    }
```

#### Renaming of fields

In case the field names in derived data type are different we can use field attribute `#[rename_field("")]`


```rust
    #[derive(golem_rust::WIT_From_Into))]
    #[wit_type_name(WitPerson)]
    pub struct Person {

        #[rename_field("name2")]
        pub name: String,

        #[rename_field("age2")]
        pub age: i32,
    }
```

### Enums

Very similar to structs, let's say we have the following enum data type:


```rust 
    #[derive(golem_rust::WIT_From_Into))]
    #[wit_type_name(SimilarColors)] 
    pub enum Colors {
        Red,
        White,

        #[rename_field("Yellow2")]
        Yellow,
    }

    pub enum SimilarColors {
        Red,
        White,
        Yellow2,
    }
```

Then very simply we can use `.into()` and it will compile.

```rust 
    let yellow = Colors::Yellow;

    let wit_collors: SimilarColors = yellow.into();
```

More examples can be found in `golem-rust-example/src/main.rs`

## 2. Generate WIT file from rust module.

Let's say we are building auction app powered by Golem Cloud. We would like to support some basic functionality like:
- initializing an auction
- get all auctions
- close auctions
- create a bidder
- make a bid
Also we need some data types like describing auction, bidder, result and so on.

The WIT file itself could look like this:

```
package auction:app

interface api {

    record bidder-id {
        bidder-id: string,
    }

    record auction-id {
        auction-id: string,
    }

    record auction {
        auction-id: auction-id, 
		name: string, 
		description: string, 
		starting-price: float32, 
		deadline: deadline,
    }

    variant bid-result {
        failure(string), 
 		success
    }

    type deadline = u64
                

    initialize: func(auction: auction)
                    

    bid: func(bidder-id: bidder-id, price: float32) -> bid-result
                    

    close-auction: func() -> option<bidder-id>
                    

    create-bidder: func(name: string, address: string) -> bidder-id
                    

    create-auction: func(name: string, description: string, starting-price: float32, deadline: u64) -> auction-id
                    

    get-auctions: func() -> list<auction>
                    
}

world golem-service {
    export api
}
```

There are many things that could go wrong when writing this, especially if you're not familiar with WIT. But mostly, it's just a boilerplate that can now be avoided. 

Simply annotate your inner module with  `#[golem_rust::create_wit_file]` macro.

```rust
#[golem_rust::create_wit_file]
mod auction_app {
    
    struct BidderId {
        bidder_id: String 
    }

    struct AuctionId {
        auction_id: String
    }

    struct Auction {
        auction_id: AuctionId,
        name: String, 
        description: String,
        starting_price: f32,
        deadline: Deadline,
    }

    enum BidResult {
        Failure(String),
        Success 
    }

    type Deadline = u64;

    trait AuctionService {

        fn initialize(auction: Auction);

        fn bid(bidder_id: BidderId, price: f32) -> BidResult;

        fn close_auction() -> Option<BidderId>;

        fn create_bidder(name: String, address: String) -> BidderId;

        fn create_auction(name: String, description: String, starting_price: f32, deadline: u64) -> AuctionId;

        fn get_auctions() -> Vec<Auction>;
    }
}
```

and this will generate `generated.wit` file in the root of your project. 
If you want your generated file to have custom name, add the name to the attribute e.g. `#[golem_rust::create_wit_file("auction_app_file.wit")]`

### WIT file generation details

The following empty inner module

```rust
#[golem_rust::create_wit_file]
mod package_name {

}
```

translates to the empty with file with package name derived from module name:

```
package package:name
    
interface api {

}

world golem-service {
    export api
}
```
So interface name is always `api` which is exported from `world geolem-service`

Other rules of wit file generation:
- Rust `struct` is translated into WIT `record`.
- `Enum` is translated into either `variant` or `enum` depending on whether enum has associated data. 
- `Option<>` is `option<>`.
- array and `vec<>` is `list<>`.
- type aliases `type Name = String` becomes `type name = string`
- `Box<>` is ignored and inner type is taken care of.
- tuples are supported.
- PascalCase is replaced with kebab-case. 
- snake_case is replaced with kebab-case.
- Trait name does not matter.
- Functions inside trait are translated to WIT file functions.
- It has to be inner module and all used types need to be defined inside module.
- If there are multiple traits inside module, their content is concatenated into single wit interface.

## How to contribute

Contributions very are welcome. If you find a bug, use case that is not supported or you simply think that error message is not good enough, please open an issue or submit a PR. This library is still at an early stage of development and although some use cases are covered, feedback would be very helpful for polishing this library.

## golem-rust-examples

Inner binary project which depends on golem-rust. Here you can find more examples on how to use golem-rust.