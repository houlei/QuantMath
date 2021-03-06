use std::collections::HashMap;
use std::rc::Rc;
use std::any::Any;
use core::qm;
use dates::Date;
use data::curves::RateCurve;
use data::divstream::DividendStream;
use data::volsurface::VolSurface;
use data::forward::Forward;
use data::forward::EquityForward;
use data::bump::Bump;
use data::bumpspot::BumpSpot;
use data::bumpyield::BumpYield;
use data::bumpdivs::BumpDivs;
use data::bumpvol::BumpVol;
use instruments::Instrument;
use instruments::PricingContext;
use risk::Bumpable;
use risk::Saveable;
use risk::BumpablePricingContext;

/// The market data struct contains all the market data supplied for a
/// valuation. It has methods for building the analytics needed for valuation
/// such as forwards.
///
/// All market data is identified by a single string. Where data should be
/// keyed by multiple fields, for example a yield curve is keyed by currency
/// and credit entity, there is a conventional way of combining the ids of
/// the fields to create a unique key.
///
/// As new forms of market data are required, they should be added to this
/// struct. They may also need to be added to PricingContext, so they can be
/// accessed during pricing.
#[derive(Clone)]
pub struct MarketData {
    spot_date: Date, 
    discount_date: Option<Date>, 
    spots: HashMap<String, f64>,
    yield_curves: HashMap<String, Rc<RateCurve>>,
    borrow_curves: HashMap<String, Rc<RateCurve>>,
    dividends: HashMap<String, Rc<DividendStream>>,
    vol_surfaces: HashMap<String, Rc<VolSurface>>
}

impl MarketData {
    /// Creates a market data object. There is normally only one of these
    /// supplied to any valuation.
    ///
    /// Apart from spots, all market data is allowed to have a base date in
    /// the past, in which case standard rules are used for bringing the values
    /// up to date. This is useful for valuation early in the morning, before
    /// the market has opened to give liquid option prices etc.
    ///
    /// * 'spot_date'      - The date of all the spot values. Normally today
    /// * 'discount_date'  - The date to which all valuation should be 
    ///                      discounted. If it is supplied, it is normally
    ///                      T + 2. If not supplied, all instruments are
    ///                      discounted to their own settlement date, which
    ///                      means they match their screen prices, but are
    ///                      not necessarily mutually consistent.
    /// * 'spots'          - Values of any numeric screen prices, keyed by the
    ///                      id of the instrument, such as an equity
    /// * 'yield_curves'   - Precooked yield curves, keyed by credit id
    /// * 'borrow_curves'  - Cost of borrow or repo curves, keyed by the id
    ///                      of the instrument, such as an equity
    /// * 'dividends'      - Dividend streams, keyed by the id of the equity
    /// * 'vol_surfaces'   - Vol surfaces, keyed by the id of the instrument
    ///                      such as an equity. Vol cubes for interest rates
    ///                      will be supplied as a separate entry.
    pub fn new(
        spot_date: Date, 
        discount_date: Option<Date>, 
        spots: HashMap<String, f64>,
        yield_curves: HashMap<String, Rc<RateCurve>>,
        borrow_curves: HashMap<String, Rc<RateCurve>>,
        dividends: HashMap<String, Rc<DividendStream>>,
        vol_surfaces: HashMap<String, Rc<VolSurface>>) -> MarketData {

        MarketData {
            spot_date: spot_date, 
            discount_date: discount_date,
            spots: spots,
            yield_curves: yield_curves,
            borrow_curves: borrow_curves,
            dividends: dividends,
            vol_surfaces: vol_surfaces }
    }
}

impl PricingContext for MarketData {

    fn spot_date(&self) -> Date {
        self.spot_date
    }

    fn discount_date(&self) -> Option<Date> {
        self.discount_date
    }

    fn yield_curve(&self, credit_id: &str, _high_water_mark: Date)
            -> Result<Rc<RateCurve>, qm::Error> {
        find_market_data(credit_id, &self.yield_curves, "Yield curve")
    }

    fn spot(&self, id: &str) -> Result<f64, qm::Error> {
        find_market_data(id, &self.spots, "Spot")
    }

    fn forward_curve(&self, instrument: &Instrument, high_water_mark: Date)
        -> Result<Rc<Forward>, qm::Error> {

        // This assumes the instrument is an equity. Need handling for other
        // types of underlying that may not have dividends or borrow, or may
        // be driftless
        let id = instrument.id();
        let spot = find_market_data(id, &self.spots, "Spot")?;
        let divs = find_market_data(id, &self.dividends, "Dividends")?;
        let borrow = find_market_data(id, &self.borrow_curves, "Borrow curve")?;
        
        let credit_id = instrument.credit_id();
        let yield_curve = find_market_data(&credit_id, &self.yield_curves, 
            "Yield curve for forward")?;
  
        // We create the forward on the fly. For efficiency, we could cache
        // the forward if the request is the same and there are no relevant
        // bumps
        let settlement = instrument.settlement().clone();
        let forward = EquityForward::new(self.spot_date, spot, settlement,
            yield_curve, borrow, &*divs, high_water_mark)?;
        Ok(Rc::new(forward))
    }

    /// Gets a Vol Surface, given any instrument, for example an equity.  Also
    /// specify a high water mark, beyond which we never directly ask for
    /// vols.
    fn vol_surface(&self, instrument: &Instrument, forward: Rc<Forward>,
        _high_water_mark: Date) -> Result<Rc<VolSurface>, qm::Error> {

        let id = instrument.id();
        let mut vol = find_market_data(id, &self.vol_surfaces, "Vol surface")?;

        // decorate or modify the surface to cope with any time or forward shift
        instrument.vol_time_dynamics().modify(&mut vol, self.spot_date)?; 
        instrument.vol_forward_dynamics().modify(&mut vol, forward)?;
        Ok(vol)
    }

    fn correlation(&self, _first: &Instrument, _second: &Instrument)
        -> Result<f64, qm::Error> {
        Err(qm::Error::new("Correlation not implemented"))
    }
}

fn find_market_data<T: Clone>(id: &str, collection: &HashMap<String, T>,
    item: &str) -> Result<T, qm::Error> {

    match collection.get(id) {
        None => Err(qm::Error::new(&format!(
            "{} not found: '{}'", item, id))),
        Some(x) => Ok(x.clone())
    }
} 

impl Bumpable for MarketData {

    fn bump_spot(&mut self, id: &str, bump: &BumpSpot, save: &mut Saveable)
        -> Result<bool, qm::Error> {
        let saved = to_saved_data(save)?;
        apply_bump(id, bump, &mut self.spots, &mut saved.spots)
    }

    fn bump_yield(&mut self, credit_id: &str, bump: &BumpYield,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved_data(save)?;
        apply_bump(credit_id, bump, &mut self.yield_curves, 
            &mut saved.yield_curves)
    }

    fn bump_borrow(&mut self, id: &str, bump: &BumpYield,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved_data(save)?;
        apply_bump(id, bump, &mut self.borrow_curves, &mut saved.borrow_curves)
    }

    fn bump_divs(&mut self, id: &str, bump: &BumpDivs,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved_data(save)?;
        apply_bump(id, bump, &mut self.dividends, &mut saved.dividends)
    }

    fn bump_vol(&mut self, id: &str, bump: &BumpVol,
        save: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved_data(save)?;
        apply_bump(id, bump, &mut self.vol_surfaces, &mut saved.vol_surfaces)
    }

    fn bump_discount_date(&mut self, replacement: Date, save: &mut Saveable)
        -> Result<bool, qm::Error> {
        let saved = to_saved_data(save)?;
        saved.discount_date = self.discount_date;
        self.discount_date = Some(replacement);
        saved.replaced_discount_date = true;
        Ok(saved.discount_date != self.discount_date)
    }

    fn forward_id_by_credit_id(&self, _credit_id: &str) 
        -> Result<&[String], qm::Error> {
        Err(qm::Error::new("Forward id from credit id mapping not available \
            you need to use PrefetchedPricingContext"))
    }

    fn new_saveable(&self) -> Box<Saveable> {
        Box::new(SavedData::new())
    }

    fn restore(&mut self, any_saved: &Saveable) -> Result<(), qm::Error> {
        
        if let Some(saved) = any_saved.as_any().downcast_ref::<SavedData>()  {

            if saved.replaced_discount_date {
                self.discount_date = saved.discount_date;
            }
            copy_from_saved(&mut self.spots, &saved.spots);
            copy_from_saved(&mut self.yield_curves, &saved.yield_curves);
            copy_from_saved(&mut self.borrow_curves, &saved.borrow_curves);
            copy_from_saved(&mut self.dividends, &saved.dividends);
            copy_from_saved(&mut self.vol_surfaces, &saved.vol_surfaces);
            Ok(())

        } else {
            Err(qm::Error::new("Mismatching save space for restore"))
        }
    }
}

impl BumpablePricingContext for MarketData {
    fn as_bumpable(&self) -> &Bumpable { self }
    fn as_mut_bumpable(&mut self) -> &mut Bumpable { self }
    fn as_pricing_context(&self) -> &PricingContext { self }
}

fn to_saved_data(save: &mut Saveable) -> Result<&mut SavedData, qm::Error> {
    if let Some(as_self) = save.as_mut_any().downcast_mut::<SavedData>()  {
        Ok(as_self)
    } else {
        Err(qm::Error::new("Mismatching save space for bump market data"))
    }
}

// local helper function to apply a bump and save the old state
fn apply_bump<T: Clone>(id: &str, bump: &Bump<T>, 
    to_bump: &mut HashMap<String, T>,
    to_save: &mut HashMap<String, T>) -> Result<bool, qm::Error> {

    // try to find the entry in the map of things to bump
    let key = id.to_string();
    if let Some(entry) = to_bump.get_mut(&key) {

        // save the old value
        to_save.insert(key, entry.clone());

        // update the new value and return true to say we changed it
        *entry = bump.apply(entry.clone());
        Ok(true)

    } else {
        // value not found, so return false to say we did not change it
        Ok(false)
    }
}

pub fn copy_from_saved<T: Clone>(to_restore: &mut HashMap<String, T>,
    saved: &HashMap<String, T>) {

    for (key, value) in saved.iter() {
        to_restore.insert(key.to_string(), value.clone());
    }
}

pub struct SavedData {
    discount_date: Option<Date>,
    replaced_discount_date: bool,
    spots: HashMap<String, f64>,
    yield_curves: HashMap<String, Rc<RateCurve>>,
    borrow_curves: HashMap<String, Rc<RateCurve>>,
    dividends: HashMap<String, Rc<DividendStream>>,
    vol_surfaces: HashMap<String, Rc<VolSurface>>
}

impl SavedData {

    /// Creates an empty market data object, which can be used for saving state
    /// so it can be restored after a bump
    pub fn new() -> SavedData {
        SavedData {
            discount_date: None,
            replaced_discount_date: false,
            spots: HashMap::new(),
            yield_curves: HashMap::new(),
            borrow_curves: HashMap::new(),
            dividends: HashMap::new(),
            vol_surfaces: HashMap::new() }
    }
}

impl Saveable for SavedData {
    fn as_any(&self) -> &Any { self }
    fn as_mut_any(&mut self) -> &mut Any { self }

    fn clear(&mut self) {
        self.discount_date = None;
        self.replaced_discount_date = false;
        self.spots.clear();
        self.yield_curves.clear();
        self.borrow_curves.clear();
        self.dividends.clear();
        self.vol_surfaces.clear();
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use instruments::assets::Currency;
    use std::rc::Rc;
    use dates::datetime::DateTime;
    use dates::datetime::TimeOfDay;
    use dates::datetime::DateDayFraction;
    use dates::rules::DateRule;
    use dates::rules::BusinessDays;
    use instruments::assets::Equity;
    use instruments::options::SpotStartingEuropean;
    use instruments::options::PutOrCall;
    use instruments::options::OptionSettlement;
    use instruments::Priceable;
    use data::divstream::DividendStream;
    use data::divstream::Dividend;
    use data::curves::RateCurve;
    use data::curves::RateCurveAct365;
    use data::volsurface::VolSurface;
    use data::volsurface::FlatVolSurface;
    use dates::calendar::WeekdayCalendar;
    use math::numerics::approx_eq;
    use math::interpolation::Extrap;

    pub fn sample_currency(step: u32) -> Currency {
        let calendar = Rc::new(WeekdayCalendar::new());
        let settlement = Rc::new(BusinessDays::new_step(calendar, step));
        Currency::new("GBP", settlement)
    }

    pub fn sample_settlement(step: u32) -> Rc<DateRule> {
        let calendar = Rc::new(WeekdayCalendar::new());
        Rc::new(BusinessDays::new_step(calendar, step))
    }

    pub fn sample_equity(currency: Rc<Currency>, step: u32) -> Equity {
        let settlement = sample_settlement(step);
        Equity::new("BP.L", "LSE", currency, settlement)
    }

    pub fn sample_european() -> Rc<SpotStartingEuropean> {

        let strike = 100.0;
        let put_or_call = PutOrCall::Call;
        let expiry = DateTime::new(
            Date::from_ymd(2018, 06, 01), TimeOfDay::Close);
        let currency = Rc::new(sample_currency(2));
        let settlement = sample_settlement(2);
        let equity = Rc::new(sample_equity(currency, 2));
        let european = SpotStartingEuropean::new("SampleEquity", "OPT",
            equity.clone(), settlement, expiry,
            strike, put_or_call, OptionSettlement::Cash).unwrap();
        Rc::new(european)
    }

    pub fn create_sample_divstream() -> Rc<DividendStream> {

        // Early divs are purely cash. Later ones are mixed cash/relative
        let d = Date::from_ymd(2017, 01, 02);
        let divs = [
            Dividend::new(1.2, 0.0, d + 28, d + 30),
            Dividend::new(0.8, 0.002, d + 210, d + 212),
            Dividend::new(0.2, 0.008, d + 392, d + 394),
            Dividend::new(0.0, 0.01, d + 574, d + 576)];

        // dividend yield for later-dated divs. Note that the base date
        // for the curve is after the last of the explicit dividends.
        let points = [(d + 365 * 2, 0.002), (d + 365 * 3, 0.004),
            (d + 365 * 5, 0.01), (d + 365 * 10, 0.015)];
        let curve = RateCurveAct365::new(d + 365 * 2, &points,
            Extrap::Zero, Extrap::Flat).unwrap();
        let div_yield = Rc::new(curve);

        Rc::new(DividendStream::new(&divs, div_yield))
    }

    pub fn create_sample_rate() -> Rc<RateCurve> {
        let d = Date::from_ymd(2016, 12, 30);
        let rate_points = [(d, 0.05), (d + 14, 0.08), (d + 182, 0.09),
            (d + 364, 0.085), (d + 728, 0.082)];
        Rc::new(RateCurveAct365::new(d, &rate_points,
            Extrap::Flat, Extrap::Flat).unwrap())
    }

    pub fn create_sample_borrow() -> Rc<RateCurve> {
        let d = Date::from_ymd(2016, 12, 30);
        let borrow_points = [(d, 0.01), (d + 196, 0.012),
            (d + 364, 0.0125), (d + 728, 0.012)];
        Rc::new(RateCurveAct365::new(d, &borrow_points,
            Extrap::Flat, Extrap::Flat).unwrap())
    }

    pub fn create_sample_flat_vol() -> Rc<VolSurface> {
        let calendar = Box::new(WeekdayCalendar());
        let base_date = Date::from_ymd(2016, 12, 30);
        let base = DateDayFraction::new(base_date, 0.2);
        Rc::new(FlatVolSurface::new(0.3, calendar, base))
    }

    pub fn sample_market_data() -> MarketData {
    
        let spot_date = Date::from_ymd(2017, 01, 02);
        let mut spots = HashMap::new();
        spots.insert("BP.L".to_string(), 100.0);
        spots.insert("GSK.L".to_string(), 200.0);

        let mut dividends = HashMap::new();
        dividends.insert("BP.L".to_string(), create_sample_divstream());
        dividends.insert("GSK.L".to_string(), create_sample_divstream());

        let mut yield_curves = HashMap::new();
        yield_curves.insert("OPT".to_string(), create_sample_rate());
        yield_curves.insert("LSE".to_string(), create_sample_rate());

        let mut borrow_curves = HashMap::new();
        borrow_curves.insert("BP.L".to_string(), create_sample_borrow());
        borrow_curves.insert("GSK.L".to_string(), create_sample_borrow());

        let mut vol_surfaces = HashMap::new();
        vol_surfaces.insert("BP.L".to_string(), create_sample_flat_vol());
        vol_surfaces.insert("GSK.L".to_string(), create_sample_flat_vol());

        MarketData::new(spot_date, None, spots, yield_curves,
            borrow_curves, dividends, vol_surfaces)
    }

    #[test]
    fn european_unbumped_price() {

        let market_data = sample_market_data();
        let european = sample_european();
        let price = european.price(&market_data).unwrap();

        // this price looks plausible, but was found simply by running the test
        assert_approx(price, 16.710717400832973, 1e-12);
    }

    #[test]
    fn european_bumped_price() {

        let market_data = sample_market_data();
        let european = sample_european();
        let unbumped_price = european.price(&market_data).unwrap();

        // clone the market data so we can modify it and also create an
        // empty saved data to save state so we can restore it
        let mut mut_data = market_data.clone();
        let mut save = SavedData::new();
                
        // now bump the spot and price. Note that this equates to roughly
        // delta of 0.5, which is what we expect for an atm option
        let bump = BumpSpot::new_relative(0.01);
        let bumped = mut_data.bump_spot("BP.L", &bump, &mut save).unwrap();
        assert!(bumped);
        let bumped_price = european.price(&mut_data).unwrap();
        assert_approx(bumped_price, 17.343905306334765, 1e-12);
      
        // when we restore, it should take the price back
        mut_data.restore(&save).unwrap();
        save.clear();
        let price = european.price(&mut_data).unwrap();
        assert_approx(price, unbumped_price, 1e-12);
                
        // now bump the vol and price. The new price is a bit larger, as
        // expected. (An atm option has roughly max vega.)
        let bump = BumpVol::new_flat_additive(0.01);
        let bumped = mut_data.bump_vol("BP.L", &bump, &mut save).unwrap();
        assert!(bumped);
        let bumped_price = european.price(&mut_data).unwrap();
        assert_approx(bumped_price, 17.13982242072566, 1e-12);
      
        // when we restore, it should take the price back
        mut_data.restore(&save).unwrap();
        save.clear();
        let price = european.price(&mut_data).unwrap();
        assert_approx(price, unbumped_price, 1e-12);
                
        // now bump the divs and price. As expected, this makes the
        // price decrease by a small amount.
        let bump = BumpDivs::new_all_relative(0.01);
        let bumped = mut_data.bump_divs("BP.L", &bump, &mut save).unwrap();
        assert!(bumped);
        let bumped_price = european.price(&mut_data).unwrap();
        assert_approx(bumped_price, 16.691032323609356, 1e-12);
      
        // when we restore, it should take the price back
        mut_data.restore(&save).unwrap();
        save.clear();
        let price = european.price(&mut_data).unwrap();
        assert_approx(price, unbumped_price, 1e-12);
                
        // now bump the yield underlying the equity and price. This
        // increases the forward, so we expect the call price to increase.
        let bump = BumpYield::new_flat_annualised(0.01);
        let bumped = mut_data.bump_yield("LSE", &bump, &mut save).unwrap();
        assert!(bumped);
        let bumped_price = european.price(&mut_data).unwrap();
        assert_approx(bumped_price, 17.525364353942656, 1e-12);
      
        // when we restore, it should take the price back
        mut_data.restore(&save).unwrap();
        save.clear();
        let price = european.price(&mut_data).unwrap();
        assert_approx(price, unbumped_price, 1e-12);
                
        // now bump the yield underlying the option and price
        let bump = BumpYield::new_flat_annualised(0.01);
        let bumped = mut_data.bump_yield("OPT", &bump, &mut save).unwrap();
        assert!(bumped);
        let bumped_price = european.price(&mut_data).unwrap();
        assert_approx(bumped_price, 16.495466805921325, 1e-12);
      
        // when we restore, it should take the price back
        mut_data.restore(&save).unwrap();
        save.clear();
        let price = european.price(&mut_data).unwrap();
        assert_approx(price, unbumped_price, 1e-12);
    }

    fn assert_approx(value: f64, expected: f64, tolerance: f64) {
        assert!(approx_eq(value, expected, tolerance),
            "value={} expected={}", value, expected);
    }
}

