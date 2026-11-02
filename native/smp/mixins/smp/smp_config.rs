use super::{
    smp_auction, smp_board, smp_casino, smp_claims, smp_economy, smp_games, smp_market, smp_nick,
    smp_places, smp_presence, smp_report, smp_shop,
};

/// `config.json` in `world/horizon-smp/`, written with its defaults the
/// first time, and again with the keys a newer version adds (values kept).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(flatten)]
    pub economy: smp_economy::Config,
    pub market: smp_market::Config,
    pub shop: smp_shop::Config,
    pub gambling: smp_games::Config,
    pub casino: smp_casino::Config,
    pub places: smp_places::Config,
    pub claims: smp_claims::Config,
    pub board: smp_board::Config,
    pub presence: smp_presence::Config,
    pub nicks: smp_nick::Config,
    pub auction: smp_auction::Config,
    pub report: smp_report::Config,
    pub web: WebPush,
}

/// `config.json`'s `web`: where the web pages (`web/market.json`,
/// `web/economy.json`) are sent for the website, `url` + `/<name>`, with
/// `token` as the bearer (off while empty). Only what changed is sent, at
/// most once a minute, gzipped, by the thread that writes the market page.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WebPush {
    pub url: String,
    pub token: String,
}

impl Default for WebPush {
    fn default() -> Self {
        Self {
            url: "https://kira-production-500e.up.railway.app/smp/ingest".into(),
            token: String::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut config = Self {
            economy: Default::default(),
            market: Default::default(),
            shop: Default::default(),
            gambling: Default::default(),
            casino: Default::default(),
            places: Default::default(),
            claims: Default::default(),
            board: Default::default(),
            presence: Default::default(),
            nicks: Default::default(),
            auction: Default::default(),
            report: Default::default(),
            web: Default::default(),
        };
        config.remove_limits();
        config
    }
}

impl Config {
    /// Apply the unlimited gameplay policy to both new and existing worlds.
    /// Monetary and timestamp ceilings below are representation safeguards.
    pub fn remove_limits(&mut self) -> bool {
        let before = serde_json::to_value(&*self).expect("serializable config");
        self.market.daily_sell_coins = 0;
        self.market.daily_sell_items = 0;
        self.market.dynamic.round_trip_hours = 0;
        self.market.dynamic.max_minute_move_percent = 0.0;
        self.gambling.min_bet = 1;
        self.gambling.max_bet = smp_economy::MAX_AMOUNT;
        self.gambling.max_tickets = smp_economy::MAX_AMOUNT;
        self.gambling.cooldown_seconds = 0;
        self.casino.min_bet = 1;
        self.casino.max_bet = smp_economy::MAX_AMOUNT;
        self.casino.daily_loss_cap = i64::MAX;
        self.casino.daily_win_cap = i64::MAX;
        self.casino.cooldown_ms = 0;
        self.places.homes = vec![[0, i64::MAX]];
        self.places.warmup_seconds = 0;
        self.places.cooldown_seconds = 0;
        self.claims.chunks = vec![[0, i64::MAX]];
        self.presence.spam_commands = 0;
        self.nicks.cooldown_seconds = 0;
        self.nicks.min_playtime_minutes = 0;
        self.auction.max_listings = usize::MAX;
        // Keep enough timestamp headroom for the current date and extensions.
        self.auction.auction_hours_max = i64::MAX / 7_200_000;
        self.auction.bid_step_percent = 0;
        before != serde_json::to_value(&*self).expect("serializable config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_world_limits_are_removed_and_migration_is_idempotent() {
        let mut c: Config = serde_json::from_value(serde_json::json!({
            "market": {"daily_sell_coins": 2000, "daily_sell_items": 640,
                "dynamic": {"round_trip_hours": 24, "max_minute_move_percent": 5}},
            "places": {"homes": [[0, 1]], "cooldown_seconds": 10},
            "gambling": {"enabled": true, "max_bet": 1000},
            "casino": {"daily_loss_cap": 10000},
            "nicks": {"ops_only": false}
        }))
        .unwrap();
        assert!(c.remove_limits());
        assert_eq!(c.market.daily_sell_coins, 0);
        assert_eq!(c.market.daily_sell_items, 0);
        assert_eq!(c.market.dynamic.round_trip_hours, 0);
        assert_eq!(c.gambling.max_bet, smp_economy::MAX_AMOUNT);
        assert_eq!(c.casino.daily_loss_cap, i64::MAX);
        assert_eq!(c.places.homes, vec![[0, i64::MAX]]);
        assert!(c.gambling.enabled);
        assert!(!c.nicks.ops_only);
        assert!(!c.remove_limits());
        let roundtrip: Config = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(roundtrip.auction.max_listings, usize::MAX);
    }
}
