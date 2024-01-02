use std::{str::FromStr, sync::Arc};

use pyth_sdk_solana::Price;
use rust_decimal::Decimal;
use tokio::sync::RwLock;

use super::cex::binance::BookTickerData;

pub struct ArbitrageFinder {
    last_found: Option<ArbitrageOpportunity>,
}

impl ArbitrageFinder {
    pub fn new() -> Self {
        return Self { last_found: None };
    }

    /*
        Compares Binance and Pyth prices to find arbitrage opportunities
    */
    pub async fn find_opportunity(
        &mut self,
        latest_pyth_price: Arc<RwLock<Option<Price>>>,
        latest_binance_ticker_data: Arc<RwLock<Option<BookTickerData>>>,
    ) -> Option<ArbitrageOpportunity> {
        let (latest_pyth_price_read, latest_binance_ticker_data_read) =
            tokio::join!(latest_pyth_price.read(), latest_binance_ticker_data.read());

        if latest_pyth_price_read.is_none() || latest_binance_ticker_data_read.is_none() {
            return None;
        }

        let pyth_price = (*latest_pyth_price_read).unwrap();
        drop(latest_pyth_price_read);
        let binance_ticker_data = (*latest_binance_ticker_data_read).clone().unwrap();
        drop(latest_binance_ticker_data_read);

        let (pyth_confident_95_price_higher, pyth_confident_95_price_lower) =
            self.get_pyth_confident_95_price(pyth_price);

        // Search for SellBinanceBuyDex opportunity
        let binance_best_bid_price = Decimal::from_str(&binance_ticker_data.b).unwrap();
        if binance_best_bid_price.gt(&pyth_confident_95_price_higher) {
            let quantity = Decimal::from_str(&binance_ticker_data.B).unwrap();
            let opportunity = ArbitrageOpportunity {
                direction: ArbitrageDirection::SellBinanceBuyDex,
                quantity,
                estimated_profit: (binance_best_bid_price - pyth_confident_95_price_higher)
                    .checked_mul(quantity)
                    .unwrap(),
                binance_price: binance_best_bid_price,
                pyth_price: pyth_confident_95_price_higher,
            };

            if let Some((last_opportunity, pyth_price, binance_price)) = self.last_found {
                if last_opportunity == opportunity
                    && pyth_price == pyth_confident_95_price_higher
                    && binance_best_bid_price == binance_price
                {
                    return None;
                }
            }
            self.last_found = Some(opportunity);

            return self.last_found;
        }

        // Search for BuyBinanceSellDex opportunity
        let binance_best_ask_price = Decimal::from_str(&binance_ticker_data.a).unwrap();
        if binance_best_ask_price.lt(&pyth_confident_95_price_lower) {
            let quantity = Decimal::from_str(&binance_ticker_data.A).unwrap();
            let opportunity = ArbitrageOpportunity {
                direction: ArbitrageDirection::BuyBinanceSellDex,
                quantity,
                estimated_profit: (pyth_confident_95_price_lower - binance_best_ask_price)
                    .checked_mul(quantity)
                    .unwrap(),
                binance_price: binance_best_ask_price,
                pyth_price: pyth_confident_95_price_lower,
            };

            if let Some((last_opportunity, pyth_price, binance_price)) = self.last_found {
                if last_opportunity == opportunity
                    && pyth_price == pyth_confident_95_price_lower
                    && binance_best_ask_price == binance_price
                {
                    return None;
                }
            }
            self.last_found = Some(opportunity);

            return self.last_found;
        }

        None
    }

    /*
        Calculates probable (95%) price using Pyth price and confidence feed and Laplace distribution
        https://docs.pyth.network/documentation/solana-price-feeds/best-practices#confidence-intervals
    */
    fn get_pyth_confident_95_price(&self, pyth_price: Price) -> (Decimal, Decimal) {
        let exponential = pyth_price.expo.abs() as u32;
        let price = Decimal::new(pyth_price.price, exponential);
        let confidence = Decimal::new(pyth_price.conf.try_into().unwrap(), exponential);
        let confidence_95 = confidence.checked_mul(Decimal::new(212, 2)).unwrap();

        (
            price.checked_add(confidence_95).unwrap(),
            price.checked_sub(confidence_95).unwrap(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArbitrageOpportunity {
    pub direction: ArbitrageDirection,
    pub quantity: Decimal,
    pub estimated_profit: Decimal,
    pub binance_price: Decimal,
    pub pyth_price: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbitrageDirection {
    SellBinanceBuyDex,
    BuyBinanceSellDex,
}

#[cfg(test)]
mod tests {
    use std::{str::FromStr, sync::Arc};

    use pyth_sdk_solana::Price;
    use rust_decimal::Decimal;
    use tokio::sync::RwLock;

    use crate::structs::cex::binance::BookTickerData;

    use super::{ArbitrageDirection, ArbitrageFinder};

    #[test]
    fn test_get_pyth_confident_95_price() {
        let arbitrage_finder = ArbitrageFinder::new();
        let price = Price {
            price: 4856126854,
            conf: 612455,
            expo: -5,
            ..Default::default()
        };

        let (higher, lower) = arbitrage_finder.get_pyth_confident_95_price(price);
        assert_eq!(lower.normalize().to_string(), "48548.284494");
        assert_eq!(higher.normalize().to_string(), "48574.252586");
    }

    #[tokio::test]
    async fn test_find_opportunity_data_none() {
        let arbitrage_finder = ArbitrageFinder::new();

        // Both none
        {
            let result = arbitrage_finder
                .find_opportunity(Arc::new(RwLock::new(None)), Arc::new(RwLock::new(None)))
                .await;
            assert!(result.is_none());
        }

        // Only binance data none
        {
            let result = arbitrage_finder
                .find_opportunity(
                    Arc::new(RwLock::new(Some(Price::default()))),
                    Arc::new(RwLock::new(None)),
                )
                .await;
            assert!(result.is_none());
        }

        // Only pyth data none
        {
            let result = arbitrage_finder
                .find_opportunity(
                    Arc::new(RwLock::new(None)),
                    Arc::new(RwLock::new(Some(BookTickerData::default()))),
                )
                .await;
            assert!(result.is_none());
        }
    }

    #[tokio::test]
    async fn test_find_opportunity() {
        let arbitrage_finder = ArbitrageFinder::new();

        // SellBinanceBuyDex direction
        {
            // l: 68.43263012 h: 71.27225988
            let latest_pyth_price = Arc::new(RwLock::new(Some(Price {
                price: 69852445,
                conf: 669724,
                expo: -6,
                ..Default::default()
            })));
            let latest_binance_ticker_data = Arc::new(RwLock::new(Some(BookTickerData {
                b: "71.2833".to_string(),
                B: "0.8574".to_string(),
                a: "72.0012".to_string(),
                A: "0.9245".to_string(),
                ..Default::default()
            })));

            let result = arbitrage_finder
                .find_opportunity(latest_pyth_price, latest_binance_ticker_data)
                .await
                .unwrap();
            assert_eq!(result.direction, ArbitrageDirection::SellBinanceBuyDex);
            assert_eq!(result.quantity, Decimal::from_str("0.8574").unwrap());
            assert_eq!(
                result.estimated_profit.normalize(),
                Decimal::from_str("0.009465798888").unwrap()
            );
        }

        // BuyBinanceSellDex direction
        {
            // l: 68.43263012 h: 71.27225988
            let latest_pyth_price = Arc::new(RwLock::new(Some(Price {
                price: 69852445,
                conf: 669724,
                expo: -6,
                ..Default::default()
            })));
            let latest_binance_ticker_data = Arc::new(RwLock::new(Some(BookTickerData {
                b: "67.5421".to_string(),
                B: "1.1258".to_string(),
                a: "67.8423".to_string(),
                A: "2.5569".to_string(),
                ..Default::default()
            })));

            let result = arbitrage_finder
                .find_opportunity(latest_pyth_price, latest_binance_ticker_data)
                .await
                .unwrap();
            assert_eq!(result.direction, ArbitrageDirection::BuyBinanceSellDex);
            assert_eq!(result.quantity, Decimal::from_str("2.5569").unwrap());
            assert_eq!(
                result.estimated_profit.normalize(),
                Decimal::from_str("1.509415083828").unwrap()
            );
        }

        // No opportunity found
        {
            // l: 68.43263012 h: 71.27225988
            let latest_pyth_price = Arc::new(RwLock::new(Some(Price {
                price: 69852445,
                conf: 669724,
                expo: -6,
                ..Default::default()
            })));
            let latest_binance_ticker_data = Arc::new(RwLock::new(Some(BookTickerData {
                b: "69.2222".to_string(),
                B: "1.1258".to_string(),
                a: "69.1111".to_string(),
                A: "2.5569".to_string(),
                ..Default::default()
            })));

            let result = arbitrage_finder
                .find_opportunity(latest_pyth_price, latest_binance_ticker_data)
                .await;
            assert!(result.is_none());
        }
    }
}