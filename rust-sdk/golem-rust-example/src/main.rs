
use golem_rust::derive_wit;
use golem_rust::wit_file;
use golem_rust::WIT;


// #[wit_file]
// trait AuctionApi {
//     fn close_auction() -> Option<String>;
// }


fn main() {

    let me = Person {
        name: "Jaro".to_owned(),
        age: 32
    };

    let converted: WitPerson = me.into();

    // let black = Colors::Black;

    // let witCollors: WitColors = black.into();

    // let bid = BidResult::Someone { name: "hey".to_string(), age: 100 };


    eprintln!("hello there {} {}", converted.name2, converted.age);
}




#[derive(WIT)]
//#[wit(WitPerson)]
//#[derive_wit(WitPerson, name => name2)]
pub struct Person {
    
    #[wit(rename = "name2", tpe = "aaa")]
    pub name: String,
    
    pub age: i32
}


pub struct WitPerson {
 //   pub id: String,
    pub name2: String,
    pub age: i32
}

// #[derive(WIT)]
// #[wit(WitColors)]
pub enum Colors {
    Red,
    White,
    Black
}

pub enum WitColors {
    Red,
    White,
    Black
}


// #[derive(WIT)]
// #[wit(WitBidResult)]
pub enum BidResult {

  //  #[wit(rename = "Success2")]
    Success,

    //#[wit(rename = "Failure2")]
    Failure(String, u32),

    //#[wit(rename = "Someone2")]
    Someone{name: String, age: u32},
}
pub enum WitBidResult {
    Failure2(String, u32),
    Someone2{name: String, age: u32},
    Success2
}