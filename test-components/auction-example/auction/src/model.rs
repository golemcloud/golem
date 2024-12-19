use std::time::Duration;
use uuid::Uuid;

use crate::exports::auction::auction_exports::api::{
    Auction as WitAuction, AuctionId as WitAuctionId, BidResult as WitBidResult,
    BidderId as WitBidderId, Deadline as WitDeadline,
};

#[derive(Clone, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct BidderId {
    pub bidder_id: Uuid,
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

pub enum BidResult {
    AuctionExpired,
    PriceTooLow,
    Success,
}

impl From<WitBidResult> for BidResult {
    fn from(wit_bid_result: WitBidResult) -> Self {
        match wit_bid_result {
            WitBidResult::AuctionExpired => BidResult::AuctionExpired,
            WitBidResult::PriceTooLow => BidResult::PriceTooLow,
            WitBidResult::Success => BidResult::Success,
        }
    }
}

impl Into<WitBidResult> for BidResult {
    fn into(self) -> WitBidResult {
        match self {
            BidResult::AuctionExpired => WitBidResult::AuctionExpired,
            BidResult::PriceTooLow => WitBidResult::PriceTooLow,
            BidResult::Success => WitBidResult::Success,
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

pub struct State {
    pub auction: Option<Auction>,
    pub winning_bid: Option<(BidderId, f32)>,
}

impl State {
    pub fn new() -> Self {
        State {
            auction: None,
            winning_bid: None,
        }
    }
}
