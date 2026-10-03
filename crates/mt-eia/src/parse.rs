//! EIA spreadsheet parsers (pure: bytes in, value out).

use calamine::{Data, Reader};
use chrono::{Duration, NaiveDate};
use mt_core::SpotPrices;
use mt_data::FetchError;

/// Excel's day zero for serial dates (1900 date system, after its leap-year bug).
fn excel_date(serial: f64) -> Option<NaiveDate> {
    let base = NaiveDate::from_ymd_opt(1899, 12, 30)?;
    base.checked_add_signed(Duration::days(serial.floor() as i64))
}

/// A history workbook from EIA's `dnav/.../hist_xls/` series: a `Data 1`
/// sheet with a `Date` header row, then one row per day (an Excel date and a
/// value).
pub fn parse_spot_history(name: &str, unit: &str, bytes: &[u8]) -> Result<SpotPrices, FetchError> {
    let what = || format!("{name} spot prices");
    let mut book = calamine::Xls::new(std::io::Cursor::new(bytes))
        .map_err(|e| FetchError::parse(what(), e))?;
    let sheet = book
        .worksheet_range("Data 1")
        .map_err(|e| FetchError::parse(what(), e))?;
    let rows: Vec<&[Data]> = sheet.rows().collect();
    let header = rows
        .iter()
        .position(|r| matches!(r.first(), Some(Data::String(s)) if s.trim() == "Date"))
        .ok_or_else(|| FetchError::parse(what(), "no Date header"))?;
    let mut points: Vec<(NaiveDate, f64)> = rows[header + 1..]
        .iter()
        .filter_map(|r| {
            let day = match r.first()? {
                Data::Float(f) => excel_date(*f)?,
                Data::Int(i) => excel_date(*i as f64)?,
                Data::DateTime(d) => excel_date(d.as_f64())?,
                _ => return None,
            };
            let value = match r.get(1)? {
                Data::Float(f) => *f,
                Data::Int(i) => *i as f64,
                _ => return None,
            };
            value.is_finite().then_some((day, value))
        })
        .collect();
    points.sort_by_key(|p| p.0);
    Ok(SpotPrices {
        name: name.to_owned(),
        unit: unit.to_owned(),
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_serials_are_dates() {
        assert_eq!(excel_date(46294.0), NaiveDate::from_ymd_opt(2026, 9, 29));
        assert_eq!(excel_date(35437.0), NaiveDate::from_ymd_opt(1997, 1, 7));
    }
}
