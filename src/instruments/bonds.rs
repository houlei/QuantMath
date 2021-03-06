use std::rc::Rc;
use std::fmt::Display;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use instruments::Instrument;
use instruments::Priceable;
use instruments::PricingContext;
use instruments::DependencyContext;
use instruments::SpotRequirement;
use instruments::assets::Currency;
use dates::Date;
use dates::rules::DateRule;
use core::qm;

/// Represents a currency to be paid at a specific date.

#[derive(Clone, Debug)]
pub struct ZeroCoupon {
    id: String,
    credit_id: String,
    currency: Rc<Currency>,
    payment_date: Date,
    settlement: Rc<DateRule>
}

impl ZeroCoupon {
    /// Creates a zero coupon bond. It must have an id that uniquely
    /// represents it. It is discounted according to the yield curve
    /// matching its credit_id: it can therefore represent a risky
    /// bond. It pays on its payment date, but a settlement rule must
    /// be supplied in case the user does not pass in a discount date
    /// to discount to. Normally, the settlement rule should be that of
    /// the instrument that span off the zero coupon.
    pub fn new(id: &str, credit_id: &str, currency: Rc<Currency>,
        payment_date: Date, settlement: Rc<DateRule>) -> ZeroCoupon {

        ZeroCoupon { id: id.to_string(), credit_id: credit_id.to_string(),
            currency: currency, payment_date: payment_date,
            settlement: settlement }
    }
}

impl Instrument for ZeroCoupon {
    fn id(&self) -> &str {
        &self.id
    }

    fn payoff_currency(&self) -> &Currency {
        &*self.currency
    }

    fn credit_id(&self) -> &str {
        &self.credit_id
    }

    fn settlement(&self) -> &Rc<DateRule> {
        // A settlement period for a zero coupon does not really make sense,
        // as they have explicit settlement dates. However, we need to supply
        // one in case the user supplies a discount date of None.
        &self.settlement
    }

    fn dependencies(&self, context: &mut DependencyContext)
        -> SpotRequirement {

        context.yield_curve(&self.credit_id, self.payment_date);
        
        // for a zero coupon, the spot is always one
        // (in units of its own currency)
        SpotRequirement::NotRequired
    }

    fn is_pure_rates(&self) -> bool {
        true
    }

    fn as_priceable(&self) -> Option<&Priceable> {
        Some(self)
    }
}

impl Display for ZeroCoupon {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.id.fmt(f)
    }
}

impl PartialEq for ZeroCoupon {
    fn eq(&self, other: &ZeroCoupon) -> bool {
        self.id == other.id
    }
}    

impl Eq for ZeroCoupon {}

impl Hash for ZeroCoupon {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Priceable for ZeroCoupon {
    fn as_instrument(&self) -> &Instrument { self }

    /// Currency is worth one currency unit, but only if we are discounting
    /// to the date which is when we would receive the currency.
    fn price(&self, context: &PricingContext) -> Result<f64, qm::Error> {

        let discount_date = match context.discount_date() {
            None => {
                let spot_date = context.spot_date();
                self.settlement().apply(spot_date) },
            Some(discount_date) => discount_date };

        if discount_date == self.payment_date {
            Ok(1.0)
        } else {
            let yc = context.yield_curve(self.credit_id(),
                discount_date.max(self.payment_date))?;
            yc.df(self.payment_date, discount_date)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::numerics::approx_eq;
    use math::interpolation::Extrap;
    use data::curves::RateCurveAct365;
    use data::curves::RateCurve;
    use data::forward::Forward;
    use data::volsurface::VolSurface;
    use dates::calendar::WeekdayCalendar;
    use dates::rules::BusinessDays;
    use dates::Date;

    fn sample_currency(step: u32) -> Currency {
        let calendar = Rc::new(WeekdayCalendar::new());
        let settlement = Rc::new(BusinessDays::new_step(calendar, step));
        Currency::new("GBP", settlement)
    }

    fn sample_zero_coupon(currency: Rc<Currency>, step: u32) -> ZeroCoupon {
        let calendar = Rc::new(WeekdayCalendar::new());
        let settlement = Rc::new(BusinessDays::new_step(calendar, step));
        ZeroCoupon::new("GBP.2018-07-05", "OPT", currency,
            Date::from_ymd(2018, 07, 05), settlement)
    }

    struct SamplePricingContext { 
        discount_date: Option<Date>
    }

    impl PricingContext for SamplePricingContext {
        fn spot_date(&self) -> Date {
            Date::from_ymd(2018, 06, 01)
        }

        fn discount_date(&self) -> Option<Date> {
            self.discount_date
        }

        fn yield_curve(&self, _credit_id: &str,
            _high_water_mark: Date) -> Result<Rc<RateCurve>, qm::Error> {

            let d = Date::from_ymd(2018, 05, 30);
            let points = [(d, 0.05), (d + 14, 0.08), (d + 56, 0.09),
                (d + 112, 0.085), (d + 224, 0.082)];
            let c = RateCurveAct365::new(d, &points,
                Extrap::Flat, Extrap::Flat)?;
            Ok(Rc::new(c))
        }

        fn spot(&self, _id: &str) -> Result<f64, qm::Error> {
            Err(qm::Error::new("Spot not supported"))
        }

        fn forward_curve(&self, _instrument: &Instrument, 
            _high_water_mark: Date) -> Result<Rc<Forward>, qm::Error> {
            Err(qm::Error::new("Forward not supported"))
        }

        fn vol_surface(&self, _instrument: &Instrument, _forward: Rc<Forward>,
            _high_water_mark: Date) -> Result<Rc<VolSurface>, qm::Error> {
            Err(qm::Error::new("VolSurface not supported"))
        }

        fn correlation(&self, _first: &Instrument, _second: &Instrument)
            -> Result<f64, qm::Error> {
            Err(qm::Error::new("correlation not supported"))
        }
    }

    fn sample_pricing_context(discount_date: Option<Date>)
        -> SamplePricingContext {
        SamplePricingContext { discount_date }
    }

    #[test]
    fn zero_coupon_with_discount_date() {
        let discount_date = Some(Date::from_ymd(2018, 06, 05));
        let currency = Rc::new(sample_currency(2));
        let zero = sample_zero_coupon(currency, 2);
        let context = sample_pricing_context(discount_date);
        let price = zero.price(&context).unwrap();
        assert_approx(price, 0.9926533426860358);
    }

    #[test]
    fn zero_coupon_without_discount_date() {
        let currency = Rc::new(sample_currency(2));
        let zero = sample_zero_coupon(currency, 2);
        let context = sample_pricing_context(None);
        let price = zero.price(&context).unwrap();
        assert_approx(price, 0.9926533426860358);
    }

    fn assert_approx(value: f64, expected: f64) {
        assert!(approx_eq(value, expected, 1e-12),
            "value={} expected={}", value, expected);
    }
}
