---
title: Reading an Instant from text, and Encode, Decode and Shaped for Instant
authors: [somnicattus]
status: draft
pr: 730
issue:
decided:
touches: [lib.layers, api.std.instant, api.std.dynamic, api.std.shape]
requires: []
related: []
---

# Reading an Instant from text, and Encode, Decode and Shaped for Instant

## Summary

Add `Instant.parse(s: String) -> Instant fails ParseError`. It reads a date and time with an offset from UTC, in the form that `Instant.to_string` writes and the `date-time` of RFC 3339, extended with a space between the date and the time, the offsets `±HHMM` and `±HH`, and a space before a numeric offset, so that the timestamps written by PostgreSQL and SQL Server can be read too. The offset is required. Instant also gets Encode, Decode and Shaped, written and read as the text of `Instant.to_string` and `Instant.parse`. The range (an Int64 of milliseconds) and the unit of Instant do not change.

## Motivation

There is no way in std to make an Instant from text. An Instant comes only from `Clock.now` and `Instant.from_unix_milliseconds` (`[api.std.instant]`); `Instant.to_string` writes one, but nothing reads one back.

- A timestamp in a JSON API, a log or a configuration file (`"2025-10-12T03:04:05.678Z"`) cannot become an Instant without the user writing the calendar arithmetic (leap years, the days of each month, offsets) by hand.
- Instant has no Encode or Decode, so a record with an Instant field cannot derive them (E0326, checked with `emela check` of 0.28.1). Instant is an opaque type of std, so a user cannot write those impls either (E0331, `[trait.impl]`); the only way is to wrap it in a type of one's own.

```emel
import Std.Instant.{Instant}

type Event(name: String, at: Instant)
  derive Encode, Decode   # E0326 today: Instant implements neither
```

`Instant.to_string` was written, when Instant was added (#154), to give the same text as `Date.prototype.toISOString` of JS, and its unit, milliseconds, was chosen to match `Date.now()` (the notes of `emela-core` at that time). Reading the same form keeps text passed between Emela and a JS host the same in both directions.

Besides JS, many systems write timestamps with an offset, each with small differences in the separator and in the offset:

| Written by | Example |
|---|---|
| `toISOString` and `toJSON` of JS | `2025-10-12T03:04:05.678Z` |
| Writers of the `date-time` of RFC 3339 (`isoformat()` of Python, GNU `date -Iseconds`, `%aI` of git) | `2025-10-12T12:04:05.678000+09:00` |
| `str()` of a Python `datetime`, GNU `date --rfc-3339` | `2025-10-12 12:04:05.678000+09:00` |
| `timestamptz` of PostgreSQL | `2025-10-12 12:04:05.678+09` |
| `datetimeoffset` of SQL Server | `2025-10-12 12:04:05.6780000 +09:00` |
| `%ai` of git, `Time#to_s` of Ruby | `2025-10-12 12:04:05 +0900` |

`parse` is written in Emela. The native target (C11 and POSIX.1-2008) has no portable function that reads a date and time with an offset: `strptime` is missing from the C runtime of Windows, the `%z` of `strptime` is not in POSIX.1-2008, and neither is `timegm`, which turns a UTC `struct tm` into a time. `Date.parse` of JS is not used either: ECMA-262 leaves inputs outside its Date Time String Format to each engine, and Node 24 (V8) reads `2025-02-29T00:00:00.000Z` as March 1, reads `2025-10-12 03:04:05` (no offset) in the local time zone, and reads `2025-10-12  03:04:05Z` (two spaces). Written in Emela, `parse` gives the same result on the three targets.

## Proposal

```emel
import Std.Instant.{Instant}

@test
fn reads_what_to_string_writes() {
  t = Instant.parse("2025-10-12T03:04:05.678Z")
  assert t == Instant.from_unix_milliseconds(1760238245678)
  assert Instant.parse("2025-10-12T12:04:05.678+09:00") == t
  assert Instant.parse("2025-10-12 12:04:05.678+09") == t
  assert Instant.parse(t.to_string()) == t
}
```

### Meaning

- `parse` reads a date and time and returns the Instant of that point in time. Text that does not have the form, a date that does not exist (`2025-02-29`) and a point outside the range of Instant fail with `ParseError(input: s)`. The name and the error follow `[lib.api-rules]`: `parse` is a conversion from a string that can fail, and malformed input fails with the Prelude's ParseError.
- The form is given by the grammar in Specification changes. In short:
  - The date and the time are separated by `T`, `t` or one space.
  - The offset is required: `Z`, `z`, `±HH:MM`, `±HHMM` or `±HH`. One space may come before a numeric offset. A date and time without an offset, and a date alone, are not read: Emela has no time zones, so the point in time they denote is not known. Names of time zones (`JST`, `[Asia/Tokyo]`) are not read.
  - The hour and the minute are required; the second may be left out (`2025-10-12T03:04+09:00`), and is then 00. The fraction of a second may be left out, or have any number of digits; digits after the third are dropped, which rounds toward the past. A time with the hour alone (`2025-10-12T03Z`) is not read.
  - The year has 4 digits, or a sign and 6 or more digits, the form in which `to_string` writes a year outside 0 to 9999. Years of 7 or more digits are read, so everything that `to_string` writes can be read back.
  - A leap second (`:60`) and `24:00` are not read. Instant does not count leap seconds (`[lib.layers]`).
  - The decimal sign of the fraction is `.` only. The comma is reserved for the decimal sign: it is not read yet, and is not given another meaning, so that it can be read as the decimal sign later.
- Text of more than 256 code points fails before it is read. The longest text that `to_string` writes has 30 code points, and the longest of the forms in Motivation has fewer than 40.
- For every Instant `t`, `parse(to_string(t)) == t`.
- Encode writes `Text(Instant.to_string(t))`. Decode reads Text with `Instant.parse`. Shaped is the shape of String with the note `format: "date-time"`, inside `Named("Instant", ...)`. The note describes what Encode writes, which is a `date-time` of RFC 3339 for the years 0 to 9999. Decode reads more forms than that, and an Instant outside those years does not fit the note; notes describe and do not restrict values (`[api.std.shape]`), so the note stays.

### Typing rules

No new typing rules.

## Specification changes

### `[lib.layers]`

In the bullet on Instant and Duration, the sentence on creating an Instant becomes:

> An Instant can be created with `Instant.from_unix_milliseconds(ms: Int64)` (to fix the time in a test, or when a user writes a Clock handler), and read from text with `Instant.parse` (`[api.std.instant]`).

### `[api.std.instant]`

Add a row to the table, after `to_string`:

> | `parse` | `parse(s: String) -> Instant fails ParseError` | Emela | The point in time written in `s`, in the form below. Fails with `ParseError(input: s)` otherwise |

Add after the bullets on `to_string`:

> - `parse` reads a date and time with an offset from UTC, by this grammar. The whole of `s` MUST match it; no whitespace is allowed around it.
>
>   ```ebnf
>   date_time = date , ( "T" | "t" | " " ) , time , offset ;
>   date      = year , "-" , digit2 , "-" , digit2 ;
>   year      = digit4 | sign , digit6 , { digit } ;
>   time      = digit2 , ":" , digit2 , [ ":" , digit2 , [ "." , digit , { digit } ] ] ;
>   offset    = "Z" | "z" | [ " " ] , sign , digit2 , [ [ ":" ] , digit2 ] ;
>   sign      = "+" | "-" ;
>   digit6    = digit2 , digit2 , digit2 ;
>   digit4    = digit2 , digit2 ;
>   digit2    = digit , digit ;
>   digit     = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
>   ```
>
> - The fields are read as the date and the time of day at the given offset, in the proleptic Gregorian calendar. The month is 01 to 12, the day is 01 to the number of days of that month in that year, the hour is 00 to 23, the minute is 00 to 59, and the second is 00 to 59; an omitted second is 00. The hour of the offset is 00 to 23 and its minute is 00 to 59; an offset written with the hour only (`+09`) has the minute 00. `Z`, `z`, and an offset whose hour and minute are both 00 (`+00:00`, `-0000`, `+00`) are the offset 0.
> - If `s` has more than 256 code points, `parse` fails without reading it.
> - The separator between the date and the time, and the space before the offset, are one space (U+0020). No other whitespace is allowed.
> - A year written with a sign is that many years after (`+`) or before (`-`) the year 0. The sign `-` with the value 0 (`-000000`) is not read. A year of 4 digits is 0000 to 9999.
> - A comma is reserved for the decimal sign of the fraction (`03:04:05,678`). `parse` does not read it yet, and a comma is not given any other meaning in this form.
> - The fraction of a second is read to the millisecond: the first three digits are the milliseconds (fewer digits are padded with zeros on the right), and the digits after the third are dropped. The result is the latest millisecond that is not later than the time written.
> - The point in time is the date and time minus the offset. If it is outside the range of Instant (an Int64 of milliseconds), `parse` fails. The fields are read as exact integers: a year of any number of digits (with any number of leading zeros) that is outside the range fails, and does not wrap around by the rules of Int64. The milliseconds depend only on the first three digits of the fraction, however many digits follow.
> - For every Instant `t`, `parse(to_string(t)) == t`.
> - `parse` is O(n) in the length of `s`.

Change the impl paragraph to:

> impl (`[trait.derive]`, `[trait.builtin]`): `derive Eq, Ord`. The Show impl is the same as `to_string`. Encode, Decode and Shaped are in `[api.std.dynamic]` and `[api.std.shape]`.

### `[api.std.dynamic]`

In the impl paragraph, add Instant to the list of types that have Encode and Decode, and add:

> An Instant is encoded as Text of `Instant.to_string`, and decoded from Text with `Instant.parse`. A value that is not Text fails with `DecodeError(expected: "text", ...)`, and Text that `Instant.parse` does not read fails with `DecodeError(expected: "date-time", actual: the value, path: [])`.

### `[api.std.shape]`

In the impl paragraph, add Instant to the list of types that have Shaped, and add:

> The shape of Instant is `Named("Instant", Shape.format(Shape.String, "date-time"))`.

## Diagnostics

- [x] None.

## Compatibility

No program that compiles now changes meaning.

- `Std.Instant` gains the pub fn `parse`. A method call `s.parse()` looks up the fns of the module that defines String (`[type.rules]`), so existing calls do not become ambiguous.
- Instant gains Encode, Decode and Shaped. Users could not write these impls (E0331), so no program becomes E0330.
- The output of `to_string` does not change, including years outside 0 to 9999.
- Each restriction in Alternatives (no comma, no leap second, no hour alone, no offset seconds, no time zone names) and the limit of 256 code points can be lifted later without changing the result of any text that `parse` reads now.

## Implementation

- emela-core (std sources): write `parse` in Emela in `std/instant.emel`, as the inverse of the civil_from_days of `to_string` (days_from_civil). Add the impls for Instant to `std/dynamic.emel` and `std/shape.emel`, the modules of the traits, where the impls for Bytes, Array, Map and Set are (`[trait.impl]` allows either module; users see no difference).
- No change to the compiler.
- Tests: `parse(to_string(t)) == t` at the epoch, -1 ms, a leap day, the years 0, -1, 9999 and 10000, and both ends of Int64. Every form in the table of Motivation reads as the same Instant. A time without seconds reads as 00 seconds. Text of exactly 256 code points that is read (a long fraction), and text of 257 code points that fails. Failures for a date that does not exist, `:60`, `24:00`, no offset, a date alone, the hour alone, two spaces, whitespace around the text, a tab, `±HH:MM:SS`, a time zone name and a comma. The first and the last point of Instant written with every offset, and the points just outside them, which fail. Years with many significant digits (more than an Int64 holds) and with many leading zeros. Fractions of 20 digits or more, and fractions that a Float does not hold exactly (`.678` is 678 milliseconds, not 677). The same results on the three targets.
- `parse` reads the code points of `s` once from the start (`String.code_points`), not with a slice per character, which would be O(n²). Each field is read into an Int64 with a bound on its digits: the year fails when its significant digits are more than 9 (Instant runs from `-292275055-05-17T16:47:04.192Z` to `+292278994-08-17T07:12:55.807Z`). The range is checked on the result, after the offset is applied: the date and time as written can lie just outside the range (the last point of Instant written with `+09:00`), so the days and the milliseconds of the day are kept apart until the check, and no intermediate value wraps around. Only the first three digits of the fraction are read as a number; the rest are checked to be digits and skipped. No Float is used.
- A check of the grammar against Node 24 (V8) and Bun 1.4.3 (JavaScriptCore), on 384 combinations of separator, offset and fraction: both engines read 275 of them as this proposal does. They do not read `±HH` or a space before the offset after `T` or `t` (108 combinations), nor a year of 7 digits (outside the range of a JS Date). Every form in the table of Motivation is read.

## Alternatives

- **Read what `Date.parse` of JS reads.** ECMA-262 leaves inputs outside its Date Time String Format to the engine (Motivation), and the same behavior cannot be written for C and WASM.
- **Read only the Date Time String Format of ECMA-262** (exactly three digits of fraction, `T` and `Z` in upper case, the offset `±HH:MM`). Everything a JS host writes is read, but the other rows of the table in Motivation are not: fractions of 0, 6, 7 and 9 digits, a space as the separator, and `±HH` and `±HHMM`.
- **Read only the `date-time` of RFC 3339.** The output of `to_string` for a year outside 0 to 9999 could not be read back, nor the forms of PostgreSQL, SQL Server and `%ai` of git.
- **Exactly three digits of fraction.** Times written to the microsecond (Python), to 100 nanoseconds (SQL Server) or to the nanosecond (GNU `date`) could not be read.
- **Require the seconds.** RFC 3339 requires them; the Date Time String Format of ECMA-262 does not, and `toString()` of `OffsetDateTime` in Java leaves them out when they and the nanoseconds are 0 (by its documentation).
- **Read the hour alone** (`2025-10-12T03Z`). ISO 8601 allows it as a time of reduced precision, but RFC 3339 and ECMA-262 do not, and no writer of it was found. Without the colon between the hour and the minute, the hour and a numeric offset stand side by side (`T03+09`), which is easy to misread.
- **Read a leap second `:60`.** It is written only by systems whose time zone counts leap seconds (`right/UTC`) and by timing equipment; `Date.parse` of V8 and `datetime.fromisoformat` of Python do not read it. Readers that accept it move it to different places (23:59:59, or 00 of the next minute). The last leap second was at the end of 2016.
- **Round the fraction to the nearest millisecond.** `23:59:59.9999Z` would become the next day. Dropping the digits keeps every field as written.
- **A comma as the decimal sign** (`03:04:05,678`). ISO 8601 allows it; RFC 3339 and ECMA-262 do not. Of the writers with an offset, only GNU `date -Ins` was found; most text with a comma is a log timestamp without an offset (the default of the `logging` module of Python). Node 24 does not read it, Python 3.13 does, and the std of Rust has no parser of dates. It can be added later; the comma is reserved for it, so that no other use takes it first.
- **Read `±HH` and a space before the offset only when the separator is a space.** V8 and JavaScriptCore read every form of this proposal with a space as the separator, but not these two after `T` or `t`. No writer of these combinations was found, and PostgreSQL and SQL Server both use a space, so the restriction would read the same real inputs. Not chosen: a separator and an offset that are chosen independently make a simpler rule, and the behavior of the engines outside the format of ECMA-262 is not a basis for the rule. What Emela passes to a JS host is the output of `to_string`, which the engines read (except years of 7 or more digits, outside the range of a JS Date).
- **Read an offset with seconds (`±HH:MM:SS`).** PostgreSQL writes one for times in the era of local mean time (`+09:18:59`). ISO 8601 has no such offset, and it appears only in that case.
- **Read time zone names** (`+0900 JST` of `String()` of Go, `[Asia/Tokyo]`). Turning a name into an offset needs a time zone database, which std does not have, and abbreviations are not unique (`IST`).
- **Read a date and time without an offset as UTC.** RFC 3339 requires the offset, and ECMA-262 reads such text in the local time zone. Either reader would see a different point in time.
- **Encode an Instant as Number** (milliseconds since the epoch). The written value cannot be told apart from other numbers (counts, IDs) by looking at it, and its unit (milliseconds or seconds) is not in the value. Text in the form of `to_string` shows that it is a time. For a peer that uses milliseconds since the epoch, a user wraps Instant in a type of their own and writes the impls (`[trait.impl]`).
- **Add only `parse`, and leave Encode, Decode and Shaped for later**, to decide how an Instant is represented in Dynamic together with ObjectKey (below). The Decode of Bytes reads only Binary and leaves the text form to the caller (`std/dynamic.emel`). `parse` does not depend on Dynamic and is useful alone (reading logs and configuration, writing the Decode of a wrapping type by hand), while the impls depend on `parse`; so only the impls could be left for later. Not chosen: meanwhile a record with an Instant field cannot derive Encode and Decode, and the representation as Text can be decided without deciding ObjectKey.
- **ObjectKey for Instant** (`to_key` by `to_string`, `from_key` by `parse`), so that `Map[Instant, V]` can be written as Fields. Not in this proposal:
  - The only basic type with ObjectKey today is String; Int and Int64 do not have it (`[api.std.dynamic]`). The ObjectKey of the basic types is better decided together.
  - Many texts denote the same Instant (`...Z` and `...+09:00`, different numbers of digits in the fraction). When Fields has two keys for the same point in time, the Decode of Map inserts the same key twice and keeps one. Fields is ordered by the text of the keys (`[api.std.map]`), so the one kept is the later in the order of the text, not the later in the document. Making it a failure needs a change to the Decode of Map.
  - It can be added later without breaking programs: users cannot write the ObjectKey of Instant (E0331). Until then, a user writes it on a wrapping type (`type At(inner: Instant)`).
- **No limit on the length of `s`.** Not chosen: no writer of a date and time comes near 256 code points, and the limit bounds the work of `parse` and the digits of each field whatever text it is given. The limit counts code points, as `Shape.max_length` does, not the grapheme clusters of `String.length`. It can be raised later without changing the result of any text that `parse` reads now.
- **A `parse` that returns Option.** `[lib.api-rules]` has no Option versions for malformed input.

## Prior art

- Text forms of dates and times: ISO 8601 (with the expanded representation of the year, offsets `±hh:mm`, `±hhmm` and `±hh`, and the comma or the full stop as the decimal sign); the `date-time` of RFC 3339, a profile of ISO 8601 (the offset required, any number of digits of fraction, `T` or a space between the date and the time by its note); the Date Time String Format of ECMA-262, a simplification of ISO 8601 (three digits of fraction, years of six digits with a sign, a date and time without an offset in the local time zone).
- The `format: "date-time"` of JSON Schema is the `date-time` of RFC 3339.

## Open questions

## Decision
