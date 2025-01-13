use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;
use crate::bindings::auction::auction_client::auction_client;

use crate::exports::auction::registry_exports::api::{
    Auction as WitAuction, AuctionId as WitAuctionId, BidderId as WitBidderId,
    Deadline as WitDeadline,
};

#[derive(Clone, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct BidderId {
    pub bidder_id: Uuid,
}

impl BidderId {
    pub fn new() -> Self {
        BidderId {
            bidder_id: Uuid::new_v4(),
        }
    }
}

impl From<WitBidderId> for BidderId {
    fn from(wit_bidder_id: WitBidderId) -> Self {
        BidderId {
            bidder_id: Uuid::parse_str(&wit_bidder_id.bidder_id).unwrap(),
        }
    }
}

impl Into<WitBidderId> for BidderId {
    fn into(self) -> WitBidderId {
        WitBidderId {
            bidder_id: self.bidder_id.to_string(),
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct AuctionId {
    pub auction_id: Uuid,
}

impl AuctionId {
    pub fn new() -> Self {
        AuctionId {
            auction_id: Uuid::new_v4(),
        }
    }
}

impl From<WitAuctionId> for AuctionId {
    fn from(wit_auction_id: WitAuctionId) -> Self {
        AuctionId {
            auction_id: Uuid::parse_str(&wit_auction_id.auction_id).unwrap(),
        }
    }
}

impl Into<WitAuctionId> for AuctionId {
    fn into(self) -> WitAuctionId {
        WitAuctionId {
            auction_id: self.auction_id.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct Deadline {
    pub deadline: Duration,
}

impl From<WitDeadline> for Deadline {
    fn from(wit_deadline: WitDeadline) -> Self {
        Deadline {
            deadline: Duration::from_secs(wit_deadline),
        }
    }
}

impl Into<WitDeadline> for Deadline {
    fn into(self) -> WitDeadline {
        self.deadline.as_secs()
    }
}

#[derive(Clone)]
pub struct Auction {
    pub auction_id: AuctionId,
    pub name: String,
    pub description: String,
    pub limit_price: f32,
    pub expiration: Deadline,
}

impl Auction {
    pub fn new(
        auction_id: AuctionId,
        name: String,
        description: String,
        limit_price: f32,
        expiration: Deadline,
    ) -> Self {
        Auction {
            auction_id,
            name,
            description,
            limit_price,
            expiration,
        }
    }
}

impl From<WitAuction> for Auction {
    fn from(wit_item: WitAuction) -> Self {
        Auction {
            auction_id: AuctionId::from(wit_item.auction_id),
            name: wit_item.name,
            description: wit_item.description,
            limit_price: wit_item.limit_price,
            expiration: Deadline::from(wit_item.expiration),
        }
    }
}

impl Into<WitAuction> for Auction {
    fn into(self) -> WitAuction {
        WitAuction {
            auction_id: self.auction_id.into(),
            name: self.name,
            description: self.description,
            limit_price: self.limit_price,
            expiration: self.expiration.into(),
        }
    }
}

#[allow(dead_code)]
pub struct Bidder {
    bidder_id: BidderId,
    name: String,
    address: String,
}

impl Bidder {
    pub fn new(bidder_id: BidderId, name: String, address: String) -> Self {
        Bidder {
            bidder_id,
            name,
            address,
        }
    }
}

pub struct State {
    pub bidders: HashMap<BidderId, Bidder>,
    pub items: HashMap<AuctionId, Auction>,
    pub actors: HashMap<AuctionId, auction_client::RunningAuction>,
}

impl State {
    pub fn new() -> Self {
        State {
            bidders: HashMap::new(),
            items: HashMap::new(),
            actors: HashMap::new(),
        }
    }
}
