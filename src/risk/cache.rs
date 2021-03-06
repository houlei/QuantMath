use std::rc::Rc;
use std::collections::HashMap;
use std::any::Any;
use data::volsurface::VolSurface;
use data::forward::Forward;
use data::curves::RateCurve;
use data::bumpspot::BumpSpot;
use data::bumpyield::BumpYield;
use data::bumpdivs::BumpDivs;
use data::bumpvol::BumpVol;
use dates::Date;
use instruments::Instrument;
use instruments::PricingContext;
use risk::dependencies::DependencyCollector;
use risk::marketdata::MarketData;
use risk::marketdata::SavedData;
use risk::marketdata::copy_from_saved;
use risk::Bumpable;
use risk::Saveable;
use risk::BumpablePricingContext;
use core::qm;

/// Use the dependencies information for a product to prefetch the market data
/// needed for calculations. Although the module is called cache, the behaviour
/// is entirely deterministic. We prefetch the data, rather than lazily caching
/// it.

pub struct PricingContextPrefetch {
    context: MarketData,
    dependencies: Rc<DependencyCollector>,
    forward_curves: HashMap<String, Rc<Forward>>,
    vol_surfaces: HashMap<String, Rc<VolSurface>>,
}

impl PricingContextPrefetch {
    /// Creates a context wrapper that prefetches forwards and potentially
    /// vol surfaces for efficiency. The MarketData context that is passed in
    /// is immediately cloned, so the PricingContextPrefetch can modify it
    /// for bumping. The dependencies that are passed in are shared and
    /// immutable.
    pub fn new(
        context: &MarketData,
        dependencies: Rc<DependencyCollector>)
        -> Result<PricingContextPrefetch, qm::Error> {

        // prefetch the forward curves and vol surfaces
        let mut forward_curves = HashMap::new();
        let mut vol_surfaces = HashMap::new(); 
        walk_dependencies(
            &context, &dependencies, &mut forward_curves, &mut vol_surfaces)?;

        Ok(PricingContextPrefetch {
            context: context.clone(),
            dependencies: dependencies,
            forward_curves: forward_curves,
            vol_surfaces: vol_surfaces
        })
    }

    /// Refetch all of the cached data after some change that affects all
    /// dependencies, such as a theta bump
    pub fn refetch_all(&mut self) -> Result<(), qm::Error> {
        self.forward_curves.clear();
        self.vol_surfaces.clear();
        walk_dependencies(
            &self.context, &self.dependencies, 
            &mut self.forward_curves, &mut self.vol_surfaces)
    }

    /// Refetch some of the cached data after a change that affects only the
    /// forward or vol surface on one instrument, such as a delta bump
    pub fn refetch(&mut self, id: &str,
        bumped_forward: bool,
        bumped_vol: bool,
        saved: &mut SavedPrefetch)
        -> Result<bool, qm::Error> {

        // if nothing was bumped, there is nothing to do (this test included
        // here to simplify usage)
        if !bumped_forward && !bumped_vol {
            return Ok(false)
        }

        // whether we are bumping vol or forward, we need the old forward
        let id_string = id.to_string();
        if let Some(fwd) = self.forward_curves.get_mut(&id_string) {

            if let Some(inst) = self.dependencies.instrument_by_id(id) {
                let instrument: &Instrument = &*inst.clone();

                // save the old forward if we are about to bump it
                if bumped_forward {
                    saved.forward_curves.insert(id.to_string(), fwd.clone());

                    // Refetch forward: requires instrument and high water mark
                    if let Some(hwm) 
                        = self.dependencies.forward_curve_hwm(inst) {
                        *fwd = self.context.forward_curve(instrument, hwm)?;
                    } else {
                        return Err(qm::Error::new("Cannot find forward"))
                    }
                }

                // If we had vol surfaces such as sticky delta surfaces that
                // needed to be updated when the forward was changed, we'd need
                // the following test to be more complicated than just 
                // looking at bumped_vol

                // save the old vol surface if we are about to bump it
                if bumped_vol {
                    if let Some(vol) = self.vol_surfaces.get_mut(&id_string) {
                        saved.vol_surfaces.insert(id_string, vol.clone());

                        // Refetch vol if required. If vol not found, it may
                        // not be an error if we are responding to a forward
                        // bump, but that code is not implemented yet.
                        if let Some(vol_hwm) = 
                            self.dependencies.vol_surface_hwm(inst) {
                            *vol = self.context.vol_surface(instrument,
                                fwd.clone(), vol_hwm)?;
                        } else {
                            return Err(qm::Error::new("Cannot find vol"))
                        }
                    }
                }
            } else {
                return Err(qm::Error::new("Cannot find instrument"))
            }
        } else {
            return Err(qm::Error::new("Cannot find prefetched forward"))
        }

        Ok(true)
    }
}

fn walk_dependencies(
    context: &MarketData,
    dependencies: &Rc<DependencyCollector>,
    forward_curves: &mut HashMap<String, Rc<Forward>>,
    vol_surfaces: &mut HashMap<String, Rc<VolSurface>>)
    -> Result<(), qm::Error> {

    let forward_dependencies = dependencies.forward_curves();
    let vol_dependencies = dependencies.vol_surfaces();

    println!("Walk dependencies. forwards={} vols={}",
        forward_dependencies.len(),
        vol_dependencies.len());

    for (rc_instrument, high_water_mark) in &*forward_dependencies {

        // fetch the forward curve
        let instrument = rc_instrument.instrument();
        let id = instrument.id().to_string();
        let forward = context.forward_curve(instrument, *high_water_mark)?;

        println!("Prefetch forward for {}", id);

        // if there is an associated vol surface, fetch that
        if let Some(vol_hwd) = vol_dependencies.get(rc_instrument) {
            let vol = context.vol_surface(instrument, forward.clone(),
                *vol_hwd)?;
            vol_surfaces.insert(id.clone(), vol);

            println!("Prefetch vol for {}", id);
        }

        forward_curves.insert(id, forward);
    }

    Ok(())
}

impl PricingContext for PricingContextPrefetch {
    fn spot_date(&self) -> Date {
        // no point caching this
        self.context.spot_date()
    }

    fn discount_date(&self) -> Option<Date> {
        // no point caching this
        self.context.discount_date()
    }

    fn yield_curve(&self, credit_id: &str, high_water_mark: Date)
        -> Result<Rc<RateCurve>, qm::Error> {
        // Currently there is no work in fetching a yield curve, so we do
        // not cache this. If yield curves were to be cooked internally, this
        // would change.
        self.context.yield_curve(credit_id, high_water_mark)
    }

    fn spot(&self, id: &str) -> Result<f64, qm::Error> {
        // no point caching this
        self.context.spot(id)
    }

    fn forward_curve(&self, instrument: &Instrument, _high_water_mark: Date)
        -> Result<Rc<Forward>, qm::Error> {
        find_cached_data(instrument.id(), &self.forward_curves, "Forward")
    }

    /// Gets a Vol Surface, given any instrument, for example an equity.  Also
    /// specify a high water mark, beyond which we never directly ask for
    /// vols.
    fn vol_surface(&self, instrument: &Instrument, _forward: Rc<Forward>,
        _high_water_mark: Date) -> Result<Rc<VolSurface>, qm::Error> {
        find_cached_data(instrument.id(), &self.vol_surfaces, "Vol Surface")
    }

    fn correlation(&self, first: &Instrument, second: &Instrument)
        -> Result<f64, qm::Error> {
        self.context.correlation(first, second)
    }
}

/// Look for market-data-derived objects in the cache. If they are not there,
/// it means that the instrument lied about its dependencies, so return an
/// error. If the high water mark mismatches, this will result in errors later
/// on when the data is used.
fn find_cached_data<T: Clone>(id: &str, collection: &HashMap<String, T>,
    item: &str) -> Result<T, qm::Error> {

    match collection.get(id) {
        None => Err(qm::Error::new(&format!(
            "{} not found (incorrect dependencies?): '{}'", item, id))),
        Some(x) => Ok(x.clone())
    }
}

impl Bumpable for PricingContextPrefetch {

    fn bump_spot(&mut self, id: &str, bump: &BumpSpot, any_saved: &mut Saveable)
        -> Result<bool, qm::Error> {
        let saved = to_saved(any_saved)?;
        let bumped = self.context.bump_spot(id, bump, &mut saved.saved_data)?;
        self.refetch(id, bumped, false, saved)
    }

    fn bump_yield(&mut self, credit_id: &str, bump: &BumpYield,
        any_saved: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved(any_saved)?;
        let bumped = self.context.bump_yield(credit_id, bump,
            &mut saved.saved_data)?;

        // we have to copy these ids to avoid a tangle with borrowing
        let v = self.dependencies.forward_id_by_credit_id(credit_id).to_vec();
        for id in v.iter() { 
            self.refetch(&id, bumped, false, saved)?;
        }

        Ok(bumped)
    }

    fn bump_borrow(&mut self, id: &str, bump: &BumpYield,
        any_saved: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved(any_saved)?;
        let bumped = self.context.bump_borrow(id, bump, &mut saved.saved_data)?;
        self.refetch(id, bumped, false, saved)
    }

    fn bump_divs(&mut self, id: &str, bump: &BumpDivs,
        any_saved: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved(any_saved)?;
        let bumped = self.context.bump_divs(id, bump, &mut saved.saved_data)?;
        self.refetch(id, bumped, false, saved)
    }

    fn bump_vol(&mut self, id: &str, bump: &BumpVol,
        any_saved: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved(any_saved)?;
        let bumped = self.context.bump_vol(id, bump, &mut saved.saved_data)?;
        self.refetch(id, false, bumped, saved)
    }

    fn bump_discount_date(&mut self, replacement: Date,
        any_saved: &mut Saveable) -> Result<bool, qm::Error> {
        let saved = to_saved(any_saved)?;
        self.context.bump_discount_date(replacement, &mut saved.saved_data)
        // the data stored here does not depend on the discount date
    }

    fn forward_id_by_credit_id(&self, credit_id: &str)
        -> Result<&[String], qm::Error> {
        Ok(self.dependencies.forward_id_by_credit_id(credit_id))
    }

    fn new_saveable(&self) -> Box<Saveable> {
        Box::new(SavedPrefetch::new())
    }

    fn restore(&mut self, any_saved: &Saveable) -> Result<(), qm::Error> {

        if let Some(saved) 
            = any_saved.as_any().downcast_ref::<SavedPrefetch>()  {

            // first restore the underlying market data
            self.context.restore(&saved.saved_data)?;

            // now restore any cached items
            copy_from_saved(&mut self.forward_curves, &saved.forward_curves);
            copy_from_saved(&mut self.vol_surfaces, &saved.vol_surfaces);
            Ok(())

        } else {
            Err(qm::Error::new("Mismatching save space for restore"))
        }
    }
}

impl BumpablePricingContext for PricingContextPrefetch {
    fn as_bumpable(&self) -> &Bumpable { self }
    fn as_mut_bumpable(&mut self) -> &mut Bumpable { self }
    fn as_pricing_context(&self) -> &PricingContext { self }
}

fn to_saved(any_saved: &mut Saveable) 
    -> Result<&mut SavedPrefetch, qm::Error> {

    if let Some(saved) 
        = any_saved.as_mut_any().downcast_mut::<SavedPrefetch>()  {
        Ok(saved)
    } else {
        Err(qm::Error::new("Mismatching save space for bumped prefetch"))
    }
}

/// Data structure for saving the prefetched content before a bump, so it
/// can be restored later on.
pub struct SavedPrefetch {
    saved_data: SavedData,
    forward_curves: HashMap<String, Rc<Forward>>,
    vol_surfaces: HashMap<String, Rc<VolSurface>>
}

impl SavedPrefetch {

    /// Creates an empty market data object, which can be used for saving state
    /// so it can be restored after a bump
    pub fn new() -> SavedPrefetch {
        SavedPrefetch {
            saved_data: SavedData::new(),
            forward_curves: HashMap::new(),
            vol_surfaces: HashMap::new() }
    }
}

impl Saveable for SavedPrefetch {
    fn as_any(&self) -> &Any { self }
    fn as_mut_any(&mut self) -> &mut Any { self }

    fn clear(&mut self) {
        self.saved_data.clear();
        self.forward_curves.clear();
        self.vol_surfaces.clear();
    }
}

// These tests are almost literally copied from marketdata.rs. They should
// behave exactly the same way, though potentially rather quicker, as the
// only effect of prefetching should be to speed things up.
#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use instruments::DependencyContext;
    use instruments::Priceable;
    use math::numerics::approx_eq;
    use risk::marketdata::tests::sample_market_data;
    use risk::marketdata::tests::sample_european;

    fn create_dependencies(instrument: &Rc<Instrument>, spot_date: Date)
        -> Rc<DependencyCollector> {

        let mut collector = DependencyCollector::new(spot_date);
        collector.spot(instrument);
        Rc::new(collector)
    }

    #[test]
    fn european_bumped_price_with_prefetch() {

        let market_data = sample_market_data();
        let european = sample_european();
        let unbumped_price = european.price(&market_data).unwrap();

        // Create a prefetch object, which internally clones the market data
        // so we can modify it and also create an
        // empty saved data to save state so we can restore it
        let spot_date = Date::from_ymd(2017, 01, 02);
        let instrument: Rc<Instrument> = european.clone();
        let dependencies = create_dependencies(&instrument, spot_date);
        let mut mut_data = PricingContextPrefetch::new(&market_data,
            dependencies).unwrap();
        let mut save = SavedPrefetch::new();

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

