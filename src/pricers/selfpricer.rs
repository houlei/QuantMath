use core::qm;
use std::rc::Rc;
use dates::Date;
use instruments::Instrument;
use instruments::PricingContext;
use instruments::DependencyContext;
use risk::cache::PricingContextPrefetch;
use risk::Pricer;
use risk::dependencies::DependencyCollector;
use risk::Bumpable;
use risk::TimeBumpable;
use risk::Saveable;
use pricers::PricerFactory;
use data::fixings::FixingTable;
use data::bumpspot::BumpSpot;
use data::bumptime::BumpTime;
use data::bumpvol::BumpVol;
use data::bumpdivs::BumpDivs;
use data::bumpyield::BumpYield;
use risk::marketdata::MarketData;

/// The SelfPricer calculator uses the Priceable interface of an
/// instrument to evaluate the instrument . It then exposes this
/// interface as a Pricer, allowing bumping for risk calculation.
pub struct SelfPricer {
    instruments: Vec<(f64, Rc<Instrument>)>,
    context: PricingContextPrefetch
}

/// The SelfPricerFactory is used to construct SelfPricer pricers.
/// It means that the interface for constructing pricers is independent of
/// what sort of pricer it is.
pub struct SelfPricerFactory {
    // no parameterisation for self-pricers
}

impl SelfPricerFactory {
    pub fn new() -> SelfPricerFactory {
        SelfPricerFactory {}
    }
}

impl PricerFactory for SelfPricerFactory {
    fn new(&self, instrument: Rc<Instrument>, fixing_table: Rc<FixingTable>, 
        market_data: Rc<MarketData>) -> Result<Box<Pricer>, qm::Error> {

        // Apply the fixings to the instrument. (This is the last time we need
        // the fixings.)
        let instruments = match instrument.fix(&*fixing_table)? {
            Some(fixed) => fixed,
            None => vec!((1.0, instrument))
        };

        // Find the dependencies of the resulting vector of instruments
        // also validate that all instruments are self-priceable
        let mut dependencies = DependencyCollector::new(
            market_data.spot_date());
        for &(_, ref instr) in instruments.iter() {
            dependencies.spot(instr);
            if let None = instr.as_priceable() {
                return Err(qm::Error::new(&format!("Instrument {} is not \
                    priceable", instr.id())))
            } 
        }

        // Create a cached pricing context, prefetching the data to price them
        let context = PricingContextPrefetch::new(&*market_data,
            Rc::new(dependencies))?;

        Ok(Box::new(SelfPricer { instruments: instruments, context: context }))
    }
}

impl Pricer for SelfPricer {
    fn as_bumpable(&self) -> &Bumpable { self }
    fn as_mut_bumpable(&mut self) -> &mut Bumpable { self }
    fn as_mut_time_bumpable(&mut self) -> &mut TimeBumpable { self }

    fn price(&self) -> Result<f64, qm::Error> {
        // Return a weighted sum of the individual prices. (TODO consider
        // returning some data structure that shows the components as well as
        // the weighted sum.)

        // Note that we have already verified that all components are priceable
        // so here we simply skip any that are not.

        let mut total = 0.0;
        for &(weight, ref instrument) in self.instruments.iter() {
            if let Some(priceable) = instrument.as_priceable() {
                total += weight * priceable.price(&self.context)?;
            }
        }
        Ok(total)
    }
}

/// There is a lot of discussion on the Rust language forum of ways to avoid
/// this braindead boilerplate.
impl Bumpable for SelfPricer {
    fn bump_spot(&mut self, id: &str, bump: &BumpSpot,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        self.context.bump_spot(id, bump, save)
    }

    fn bump_yield(&mut self, credit_id: &str, bump: &BumpYield,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        self.context.bump_yield(credit_id, bump, save)
    }

    fn bump_borrow(&mut self, id: &str, bump: &BumpYield,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        self.context.bump_borrow(id, bump, save)
    }

    fn bump_divs(&mut self, id: &str, bump: &BumpDivs,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        self.context.bump_divs(id, bump, save)
    }

    fn bump_vol(&mut self, id: &str, bump: &BumpVol,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        self.context.bump_vol(id, bump, save)
    }

    fn bump_discount_date(&mut self, replacement: Date, save: &mut Saveable)
        -> Result<bool, qm::Error> {
        self.context.bump_discount_date(replacement, save)
    }

    fn forward_id_by_credit_id(&self, credit_id: &str)
        -> Result<&[String], qm::Error> {
        self.context.forward_id_by_credit_id(credit_id)
    }

    fn new_saveable(&self) -> Box<Saveable> {
        self.context.new_saveable()
    }

    fn restore(&mut self, saved: &Saveable) -> Result<(), qm::Error> {
        self.context.restore(saved)
    }
}

impl TimeBumpable for SelfPricer {
    fn bump_time(&mut self, _bump: &BumpTime) -> Result<(), qm::Error> {
        Err(qm::Error::new("Time bumps not yet supported"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use dates::datetime::DateTime;
    use dates::datetime::TimeOfDay;
    use math::numerics::approx_eq;
    use risk::marketdata::tests::sample_market_data;
    use risk::marketdata::tests::sample_european;

    fn sample_fixings() -> FixingTable {
        let today = Date::from_ymd(2017, 01, 02);
        FixingTable::new(today, &[
            ("BP.L", &[
            (DateTime::new(today - 7, TimeOfDay::Close), 102.0)])]).unwrap()
    }

    #[test]
    fn self_price_european_bumped_price() {

        let market_data: Rc<MarketData> = Rc::new(sample_market_data());
        let instrument: Rc<Instrument> = sample_european();
        let fixings: Rc<FixingTable> = Rc::new(sample_fixings());

        let factory = SelfPricerFactory::new();
        let mut pricer = factory.new(instrument, fixings, market_data).unwrap();
        let mut save = pricer.as_bumpable().new_saveable();

        let unbumped_price = pricer.price().unwrap();
        assert_approx(unbumped_price, 16.710717400832973, 1e-12);

        // now bump the spot and price. Note that this equates to roughly
        // delta of 0.5, which is what we expect for an atm option
        let bump = BumpSpot::new_relative(0.01);
        let bumped = pricer.as_mut_bumpable().bump_spot(
            "BP.L", &bump, &mut *save).unwrap();
        assert!(bumped);
        let bumped_price = pricer.price().unwrap();
        assert_approx(bumped_price, 17.343905306334765, 1e-12);

        // when we restore, it should take the price back
        pricer.as_mut_bumpable().restore(&*save).unwrap();
        save.clear();
        let price = pricer.price().unwrap();
        assert_approx(price, unbumped_price, 1e-12);

        // now bump the vol and price. The new price is a bit larger, as
        // expected. (An atm option has roughly max vega.)
        let bump = BumpVol::new_flat_additive(0.01);
        let bumped = pricer.as_mut_bumpable().bump_vol(
            "BP.L", &bump, &mut *save).unwrap();
        assert!(bumped);
        let bumped_price = pricer.price().unwrap();
        assert_approx(bumped_price, 17.13982242072566, 1e-12);

        // when we restore, it should take the price back
        pricer.as_mut_bumpable().restore(&*save).unwrap();
        save.clear();
        let price = pricer.price().unwrap();
        assert_approx(price, unbumped_price, 1e-12);

        // now bump the divs and price. As expected, this makes the
        // price decrease by a small amount.
        let bump = BumpDivs::new_all_relative(0.01);
        let bumped = pricer.as_mut_bumpable().bump_divs(
            "BP.L", &bump, &mut *save).unwrap();
        assert!(bumped);
        let bumped_price = pricer.price().unwrap();
        assert_approx(bumped_price, 16.691032323609356, 1e-12);

        // when we restore, it should take the price back
        pricer.as_mut_bumpable().restore(&*save).unwrap();
        save.clear();
        let price = pricer.price().unwrap();
        assert_approx(price, unbumped_price, 1e-12);

        // now bump the yield underlying the equity and price. This
        // increases the forward, so we expect the call price to increase.
        let bump = BumpYield::new_flat_annualised(0.01);
        let bumped = pricer.as_mut_bumpable().bump_yield(
            "LSE", &bump, &mut *save).unwrap();
        assert!(bumped);
        let bumped_price = pricer.price().unwrap();
        assert_approx(bumped_price, 17.525364353942656, 1e-12);

        // when we restore, it should take the price back
        pricer.as_mut_bumpable().restore(&*save).unwrap();
        save.clear();
        let price = pricer.price().unwrap();
        assert_approx(price, unbumped_price, 1e-12);

        // now bump the yield underlying the option and price
        let bump = BumpYield::new_flat_annualised(0.01);
        let bumped = pricer.as_mut_bumpable().bump_yield(
            "OPT", &bump, &mut *save).unwrap();
        assert!(bumped);
        let bumped_price = pricer.price().unwrap();
        assert_approx(bumped_price, 16.495466805921325, 1e-12);

        // when we restore, it should take the price back
        pricer.as_mut_bumpable().restore(&*save).unwrap();
        save.clear();
        let price = pricer.price().unwrap();
        assert_approx(price, unbumped_price, 1e-12);
    }

    fn assert_approx(value: f64, expected: f64, tolerance: f64) {
        assert!(approx_eq(value, expected, tolerance),
            "value={} expected={}", value, expected);
    }
}
