# Golem Rust

Collection of Rust macros that facilitate working with Golem in Rust. This lib derives some boilerplate when working with WIT files and WIT derived data types.

## How to use

When working with WIT files in Golem, wit-bindgen library generates data types based on api description from the wit file. There are few drawbacks when using these data types so often the user would create its own data types. In order to easily convert between generated and domain data types, programmer needs to implement boilerplate-y `From<>` and `Into<>` typeclasses.

This project contains macros that would automatically implement those typeclasses. More examples than in this README can be found in golem-rust-example/src/main.rs

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

We can use macro help in implementing `From` and `Into` typeclasses by annotating Person with `#[derive(WIT)]` 

```rust
    #[derive(WIT)]
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

The above macro assumed that the data type for which we are deriving `From<>` and `Into<>` is called `WitPerson`. By default macro assumes that the name of the data type is `Wit` + annotated data type name. In case the name is different, we need to add `#[wit(DerivedName)]` attribute. 

```rust
    #[derive(WIT)]
    #[wit(DerivedName)]
    pub struct Person {
        pub name: String,
        pub age: i32,
    }
```

#### Renaming of fields

In case the field names in derived data type are different we can use field attribute `#[rename("")]`


```rust
    #[derive(WIT)]
    #[wit(WitPerson)]
    pub struct Person {

        #[rename("name2")]
        pub name: String,

        #[rename("age2")]
        pub age: i32,
    }
```

### Enums

Very similar to structs, let's say we have the following enum data type:


```rust 
    #[derive(WIT)]
    #[wit(SimilarColors)] 
    pub enum Colors {
        Red,
        White,

        #[rename("Yellow2")]
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

## How to contribute

Contributions very are welcome. If you find a bug, use case that is not supported or you simply think that error message is not good enough, please open an issue or submit a PR. This library is at a very early stage of development and although some use cases are covered, feedback would be very helpful for polishing this library.

Repository currently contains two separate projects:

### golem-rust

This is the library that contains macros. lib.rs file is the entry point that further delegates to the modules where macros are written.
Currently there are 2 macros:
1. Derive procedural macro in der_macro.rs which derives `From<>` and `Into<>` typeclass implementations between wit generated data types and domain data types.
2. Attribute-like procedural macro in wit_gen.rs. The idea is that user would describe WIT file interface with some Rust data type and macro could generate a wit file. This is just an idea and needs more scetching out.

### golem-rust-example

Binary project which dependes on golem-rust. Here are examples on how to use macros as well as playground for testing macro implementations.