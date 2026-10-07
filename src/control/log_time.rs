//! Native validation of Json.NET's ISO and legacy Microsoft DateTime forms.
//! Valid original text is retained; this never loads CLR or changes time zones.
//! Sources: Newtonsoft.Json13.0.3 Utilities/DateTimeParser.cs and DateTimeUtils.cs.

pub(super) fn valid(text: &str) -> bool {
    if text.is_empty() || text.len() > 64 || !text.is_ascii() { return false; }
    if let Some(body) = text.strip_prefix("/Date(").and_then(|value| value.strip_suffix(")/")) {
        let offset = body.bytes().enumerate().skip(1).find(|(_, byte)| matches!(byte,b'+'|b'-')).map(|(index,_)| index);
        let (number, zone) = offset.map_or((body,None),|at| (&body[..at],Some(&body[at..])));
        if let Some(zone) = zone {
            // Microsoft DateTime offsets select Local kind; their value is not
            // added to the epoch by Json.NET's DateTime (not DateTimeOffset) path.
            if !matches!(zone.len(),3|5) || !zone[1..].bytes().all(|byte| byte.is_ascii_digit()) { return false; }
        }
        let digits = number.strip_prefix('-').unwrap_or(number);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) { return false; }
        return number.parse::<i64>().ok().is_some_and(|milliseconds|
            (-62_135_596_800_000..=253_402_300_799_999).contains(&milliseconds));
    }
    let bytes = text.as_bytes();
    if !(19..=40).contains(&bytes.len()) || bytes[4]!=b'-' || bytes[7]!=b'-'
        || bytes[10]!=b'T' || bytes[13]!=b':' || bytes[16]!=b':' { return false; }
    let number = |start:usize,length:usize| -> Option<u32> {
        bytes.get(start..start+length)?.iter().try_fold(0,|value,byte|
            byte.is_ascii_digit().then(|| value*10+u32::from(*byte-b'0')))
    };
    let (Some(year),Some(month),Some(day),Some(hour),Some(minute),Some(second)) =
        (number(0,4),number(5,2),number(8,2),number(11,2),number(14,2),number(17,2)) else { return false; };
    if year==0 || !(1..=12).contains(&month) || minute>=60 || second>=60 || hour>24 { return false; }
    let leap = year%4==0 && (year%100!=0 || year%400==0);
    let days = match month { 2 => if leap {29} else {28}, 4|6|9|11 => 30, _ => 31 };
    if day==0 || day>days { return false; }
    let mut end = 19;
    let mut fraction_nonzero = false;
    if bytes.get(end)==Some(&b'.') {
        end+=1;
        let start=end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            fraction_nonzero |= bytes[end]!=b'0'; end+=1;
        }
        if !(1..=7).contains(&(end-start)) { return false; }
    }
    if hour==24 && (minute!=0 || second!=0 || fraction_nonzero || (year==9999 && month==12 && day==31)) { return false; }
    let zone = &bytes[end..];
    match zone {
        [] | [b'Z'|b'z'] => true,
        // DateTimeParser allows two-digit offset hours/minutes up to99; it
        // converts/clamps to local DateTime rather than imposing Offset's14h cap.
        [b'+'|b'-',a,b] => a.is_ascii_digit() && b.is_ascii_digit(),
        [b'+'|b'-',a,b,c,d] => [a,b,c,d].iter().all(|byte| byte.is_ascii_digit()),
        [b'+'|b'-',a,b,b':',c,d] => [a,b,c,d].iter().all(|byte| byte.is_ascii_digit()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_wire_forms_and_gregorian_boundaries_are_retained() {
        for text in ["0001-01-01T00:00:00", "9999-12-31T23:59:59.9999999Z",
            "2000-02-29T24:00:00.0000000Z", "2026-10-07T09:08:07.654+02:00",
            "2026-10-07T09:08:07-0230", "2026-10-07T09:08:07+02",
            "2026-10-07T09:08:07z", "/Date(0)/", "/Date(-1+0200)/",
            "/Date(-62135596800000)/", "/Date(253402300799999-02)/"] {
            assert!(valid(text),"{text}");
        }
        for text in ["", "bad", "2026-99-99T00:00:00Z", "1900-02-29T00:00:00Z",
            "0000-01-01T00:00:00Z", "9999-12-31T24:00:00Z", "2000-02-29T24:00:00.1Z",
            "2026-10-07T12:00:60Z", "2026-10-07T00:00:00.12345678Z",
            "2026-10-07T00:00:00.Z", "2026-10-07T00:00:00+2:00", "2026-10-07T00:00:00Zjunk",
            "/Date(253402300800000)/", "/Date(-62135596800001)/", "/Date(9223372036854775807)/",
            "/Date(+0)/", "/Date(0+abcd)/", "2026-10-07T00:00:00é"] {
            assert!(!valid(text),"{text}");
        }
    }
}
