use std::rc::Rc;
use std::fmt::Display;
use std::fmt;
use std::cmp::Ordering;
use std::hash::Hash;
use std::hash::Hasher;
use instruments::Instrument;
use instruments::Priceable;
use instruments::PricingContext;
use instruments::DependencyContext;
use instruments::SpotRequirement;
use dates::rules::DateRule;
use dates::datetime::TimeOfDay;
use dates::datetime::DateTime;
use dates::datetime::DateDayFraction;
use core::qm;

/// Represents a currency. Generally currencies have a one-to-one mapping with
/// world currencies. There is an exception in countries like Korea, which have
/// distinct onshore and offshore currencies, due to tradeability restrictions.
///
/// This currency always represents major units such as dollars or pounds,
/// rather than minor units such as cents or pence.

#[derive(Clone, Debug)]
pub struct Currency {
    id: String,
    settlement: Rc<DateRule>
}

impl Currency {
    pub fn new(id: &str, settlement: Rc<DateRule>) -> Currency {
        Currency { id: id.to_string(), settlement: settlement }
    }
}

impl Instrument for Currency {
    fn id(&self) -> &str {
        &self.id
    }

    fn payoff_currency(&self) -> &Currency {
        self
    }

    fn credit_id(&self) -> &str {
        // for a currency, we always take its credit id as its own name
        &self.id
    }

    fn settlement(&self) -> &Rc<DateRule> {
        &self.settlement
    }

    fn dependencies(&self, context: &mut DependencyContext)
        -> SpotRequirement {
        dependence_on_spot_discount(self, context);
        // for a currency, the spot is always one (in units of its own currency)
        SpotRequirement::NotRequired
    }

    fn as_priceable(&self) -> Option<&Priceable> {
        Some(self)
    }
}

impl Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.id.fmt(f)
    }
}

impl Ord for Currency {
    fn cmp(&self, other: &Currency) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for Currency {
    fn partial_cmp(&self, other: &Currency) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Currency {
    fn eq(&self, other: &Currency) -> bool {
        self.id == other.id
    }
}    

impl Eq for Currency {}

impl Hash for Currency {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Priceable for Currency {
    fn as_instrument(&self) -> &Instrument { self }

    /// Currency is worth one currency unit, but only if we are discounting
    /// to the date which is when we would receive the currency.
    fn price(&self, context: &PricingContext) -> Result<f64, qm::Error> {
        discount_from_spot(self, context)
    }
}

/// Simple assets are worth the screen price, but only if the date we want
/// to discount to is the same as the date when the spot price is paid.
///
/// This method calculates the discount to apply to a spot price. 

pub fn discount_from_spot(instrument: &Instrument, context: &PricingContext)
    -> Result<f64, qm::Error> {

    match context.discount_date() {
        None => Ok(1.0),
        Some(discount_date) => {
            let spot_date = context.spot_date();
            let pay_date = instrument.settlement().apply(spot_date);
            if discount_date == pay_date {
                Ok(1.0)
            } else {
                let yc = context.yield_curve(instrument.credit_id(),
                    discount_date.max(pay_date))?;
                yc.df(pay_date, discount_date)
            }
        }
    }
}

pub fn dependence_on_spot_discount(instrument: &Instrument,
    context: &mut DependencyContext) {

    // We can assume that the pricing context will provide discounts
    // at least up to its own discount date, so we do not need to specify
    // this dependency
    let spot_date = context.spot_date();
    let pay_date = instrument.settlement().apply(spot_date);
    context.yield_curve(instrument.credit_id(), pay_date);
}

/// Represents an equity single name or index. Can also be used to represent
/// funds and ETFs,

#[derive(Clone, Debug)]
pub struct Equity {
    id: String,
    credit_id: String,
    currency: Rc<Currency>,
    settlement: Rc<DateRule>
}

impl Equity {
    pub fn new(id: &str, credit_id: &str,currency: Rc<Currency>, 
        settlement: Rc<DateRule>) -> Equity {

        Equity { id: id.to_string(), credit_id: credit_id.to_string(),
            currency: currency, settlement: settlement }
    }
}

impl Instrument for Equity {
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
        &self.settlement
    }

    fn dependencies(&self, context: &mut DependencyContext)
        -> SpotRequirement {
       dependence_on_spot_discount(self, context);
       SpotRequirement::Required
    }

    fn time_to_day_fraction(&self, date_time: DateTime)
        -> Result<DateDayFraction, qm::Error> {

        // for now, we hard-code the conversion. Later we shall
        // allow this to be set per equity
        let day_fraction = match date_time.time_of_day() {
            TimeOfDay::Open => 0.0,
            TimeOfDay::EDSP => 0.0,
            TimeOfDay::Close => 0.8 };
        Ok(DateDayFraction::new(date_time.date(), day_fraction))
    }

    fn as_priceable(&self) -> Option<&Priceable> {
        Some(self)
    }
}

impl Display for Equity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.id.fmt(f)
    }
}

impl Ord for Equity {
    fn cmp(&self, other: &Equity) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for Equity {
    fn partial_cmp(&self, other: &Equity) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}   
    
impl PartialEq for Equity {
    fn eq(&self, other: &Equity) -> bool {
        self.id == other.id
    }
}

impl Eq for Equity {} 

impl Hash for Equity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Priceable for Equity {
    fn as_instrument(&self) -> &Instrument { self }

    /// The price of an equity is the current spot, but only if the date we
    /// are discounting to is the same as the spot would be paid.
    fn price(&self, context: &PricingContext) -> Result<f64, qm::Error> {
        let df = discount_from_spot(self, context)?;
        let spot = context.spot(&self.id)?;
        Ok(spot * df)
    }
}

/// Represents a credit entity
#[derive(Clone, Debug)]
pub struct CreditEntity {
    id: String,
    currency: Rc<Currency>,
    settlement: Rc<DateRule>
}

impl CreditEntity {
    pub fn new(id: &str, currency: Rc<Currency>, 
        settlement: Rc<DateRule>) -> CreditEntity {

        CreditEntity { id: id.to_string(), currency: currency,
            settlement: settlement }
    }
}

impl Instrument for CreditEntity {
    fn id(&self) -> &str {
        &self.id
    }

    fn payoff_currency(&self) -> &Currency {
        &*self.currency
    }

    fn credit_id(&self) -> &str {
        // a credit entity's id is also its credit id
        &self.id
    }

    fn settlement(&self) -> &Rc<DateRule> {
        &self.settlement
    }

    fn dependencies(&self, context: &mut DependencyContext)
        -> SpotRequirement {
       dependence_on_spot_discount(self, context);
       // for a credit entity, the spot is always one
       SpotRequirement::NotRequired
    }

    fn as_priceable(&self) -> Option<&Priceable> {
        Some(self)
    }
}

impl Display for CreditEntity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.id.fmt(f)
    }
}

impl Ord for CreditEntity {
    fn cmp(&self, other: &CreditEntity) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl PartialOrd for CreditEntity {
    fn partial_cmp(&self, other: &CreditEntity) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}   
    
impl PartialEq for CreditEntity {
    fn eq(&self, other: &CreditEntity) -> bool {
        self.id == other.id
    }
}

impl Eq for CreditEntity {} 

impl Hash for CreditEntity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Priceable for CreditEntity {
    fn as_instrument(&self) -> &Instrument { self }

    /// A credit entity is worth one currency unit, but only if we are
    /// discounting to the date which is when we would receive the currency.
    fn price(&self, context: &PricingContext) -> Result<f64, qm::Error> {
        discount_from_spot(self, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::numerics::approx_eq;
    use math::interpolation::Extrap;
    use data::forward::Forward;
    use data::volsurface::VolSurface;
    use data::curves::RateCurveAct365;
    use data::curves::RateCurve;
    use dates::calendar::WeekdayCalendar;
    use dates::rules::BusinessDays;
    use dates::Date;

    fn sample_currency(step: u32) -> Currency {
        let calendar = Rc::new(WeekdayCalendar::new());
        let settlement = Rc::new(BusinessDays::new_step(calendar, step));
        Currency::new("GBP", settlement)
    }

    fn sample_equity(currency: Rc<Currency>,
        step: u32) -> Equity {
        let calendar = Rc::new(WeekdayCalendar::new());
        let settlement = Rc::new(BusinessDays::new_step(calendar, step));
        Equity::new("BP.L", "LSE", currency, settlement)
    }

    struct SamplePricingContext { 
        spot: f64
    }

    impl PricingContext for SamplePricingContext {
        fn spot_date(&self) -> Date {
            Date::from_ymd(2018, 06, 01)
        }

        fn discount_date(&self) -> Option<Date> {
            Some(Date::from_ymd(2018, 06, 05))
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
            Ok(self.spot)
        }

        fn forward_curve(&self, _instrument: &Instrument, 
            _high_water_mark: Date) -> Result<Rc<Forward>, qm::Error> {
            Err(qm::Error::new("unsupported"))
        }

        fn vol_surface(&self, _instrument: &Instrument, _forward: Rc<Forward>,
            _high_water_mark: Date) -> Result<Rc<VolSurface>, qm::Error> {
            Err(qm::Error::new("unsupported"))
        }

        fn correlation(&self, _first: &Instrument, _second: &Instrument)
            -> Result<f64, qm::Error> {
            Err(qm::Error::new("unsupported"))
        }
    }

    fn sample_pricing_context(spot: f64) -> SamplePricingContext {
        SamplePricingContext { spot: spot }
    }

    #[test]
    fn test_equity_price_on_spot() {
        let spot = 123.4;
        let currency = Rc::new(sample_currency(2));
        let equity = sample_equity(currency, 2);
        let context = sample_pricing_context(spot);
        let price = equity.price(&context).unwrap();
        assert_approx(price, spot);
     }

    #[test]
    fn test_currency_price_on_spot() {
        let currency = sample_currency(2);
        let context = sample_pricing_context(123.4);
        let price = currency.price(&context).unwrap();
        assert_approx(price, 1.0);
    }

    #[test]
    fn test_equity_price_mismatching_dates() {
        let spot = 123.4;
        let currency = Rc::new(sample_currency(3));
        let equity = sample_equity(currency, 3);
        let context = sample_pricing_context(spot);
        let price = equity.price(&context).unwrap();

        let df = 0.9997867155076675;
        assert_approx(price, spot * df);
     }

    #[test]
    fn test_currency_price_mismatching_dates() {
        let currency = sample_currency(3);
        let context = sample_pricing_context(123.4);
        let price = currency.price(&context).unwrap();

        let df = 0.9997867155076675;
        assert_approx(price, df);
    }

    fn assert_approx(value: f64, expected: f64) {
        assert!(approx_eq(value, expected, 1e-12),
            "value={} expected={}", value, expected);
    }
}
