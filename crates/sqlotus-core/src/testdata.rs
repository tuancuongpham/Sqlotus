//! Test data: realistic rows for a table as an INSERT script. Each column
//! gets a generator chosen from its name and type (names, e-mails, phone
//! numbers, addresses, amounts, dates, …); foreign keys take values that
//! exist in the parent table and non-identity keys continue after the
//! current maximum. Deterministic for a given seed.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::catalog::{Column, DbObject, ForeignKey};
use crate::script::object_name;
use crate::sqltype::SqlType;
use crate::text::one_line;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Generator {
    /// Not inserted (identity, computed, rowversion, or excluded by the user).
    Skip,
    Null,
    FullName,
    FirstName,
    LastName,
    Email,
    Phone,
    Address,
    City,
    Country,
    Company,
    Product,
    Code,
    Status,
    Gender,
    Url,
    Text,
    Integer,
    Decimal,
    Date,
    BirthDate,
    DateTime,
    Boolean,
    Guid,
    /// Continues after the current maximum value of the column.
    Sequence,
    /// A value that exists in the referenced table.
    ForeignKey,
    /// The value the user typed, for every row.
    Fixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    #[default]
    Vi,
    En,
}

/// The generator chosen for one column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnPlan {
    pub column: String,
    pub data_type: String,
    pub nullable: bool,
    pub generator: Generator,
    /// Why it was chosen / what it produces, for the dialog.
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TestDataOptions {
    pub rows: usize,
    pub locale: Locale,
    /// Share of NULLs in nullable columns, 0–100.
    pub null_percent: u8,
    pub seed: u64,
    /// Per-column generator chosen by the user.
    pub overrides: HashMap<String, Generator>,
    /// Values for `Fixed` columns.
    pub fixed: HashMap<String, String>,
}

/// Values fetched from the database for the generators that need them.
#[derive(Debug, Clone, Default)]
pub struct DataContext {
    /// For each foreign key (by name): rows of the referenced columns.
    pub parent_rows: HashMap<String, Vec<Vec<Option<String>>>>,
    /// Current maximum per `Sequence` column (lower-case name).
    pub max_values: HashMap<String, i64>,
}

// ---------------------------------------------------------------------------
// random numbers (xorshift64*: small, deterministic, good enough)
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo + 1).max(1) as u64)) as i64
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
    fn percent(&mut self, p: u8) -> bool {
        self.below(100) < p as usize
    }
}

// ---------------------------------------------------------------------------
// word lists
// ---------------------------------------------------------------------------

const VI_LAST: &[&str] = &[
    "Nguyễn", "Trần", "Lê", "Phạm", "Hoàng", "Huỳnh", "Phan", "Vũ", "Võ", "Đặng", "Bùi", "Đỗ", "Hồ", "Ngô", "Dương",
    "Lý",
];
const VI_MIDDLE: &[&str] = &[
    "Văn", "Thị", "Hữu", "Đức", "Minh", "Ngọc", "Thanh", "Quang", "Thu", "Gia", "Hoài", "Bảo",
];
const VI_FIRST: &[&str] = &[
    "An", "Bình", "Châu", "Dũng", "Giang", "Hà", "Hải", "Hạnh", "Hiếu", "Hoa", "Hùng", "Hương", "Khánh", "Lan", "Linh",
    "Long", "Mai", "Nam", "Nga", "Ngân", "Phong", "Phúc", "Quân", "Quỳnh", "Sơn", "Tâm", "Thảo", "Trang", "Trung",
    "Tuấn", "Vy", "Yến",
];
const EN_FIRST: &[&str] = &[
    "James",
    "Mary",
    "John",
    "Patricia",
    "Robert",
    "Jennifer",
    "Michael",
    "Linda",
    "William",
    "Elizabeth",
    "David",
    "Susan",
    "Richard",
    "Jessica",
    "Joseph",
    "Sarah",
    "Thomas",
    "Karen",
    "Daniel",
    "Emma",
    "Olivia",
    "Noah",
    "Liam",
    "Ava",
];
const EN_LAST: &[&str] = &[
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis", "Wilson", "Anderson", "Taylor",
    "Thomas", "Moore", "Martin", "Jackson", "Thompson", "White", "Harris", "Clark", "Lewis",
];
const VI_CITIES: &[&str] = &[
    "Hà Nội",
    "TP. Hồ Chí Minh",
    "Đà Nẵng",
    "Hải Phòng",
    "Cần Thơ",
    "Huế",
    "Nha Trang",
    "Đà Lạt",
    "Vũng Tàu",
    "Quy Nhơn",
    "Biên Hòa",
    "Vinh",
];
const EN_CITIES: &[&str] = &[
    "New York", "London", "Seattle", "Austin", "Boston", "Chicago", "Denver", "Toronto", "Sydney", "Dublin",
    "Portland", "Atlanta",
];
const VI_STREETS: &[&str] = &[
    "Lê Lợi",
    "Trần Hưng Đạo",
    "Nguyễn Huệ",
    "Hai Bà Trưng",
    "Lý Thường Kiệt",
    "Phan Chu Trinh",
    "Điện Biên Phủ",
    "Lê Duẩn",
    "Nguyễn Trãi",
    "Cách Mạng Tháng Tám",
];
const EN_STREETS: &[&str] = &[
    "Main St",
    "Oak Ave",
    "Maple Dr",
    "Cedar Ln",
    "Park Rd",
    "Pine St",
    "Elm St",
    "Lake View Dr",
    "Hill Rd",
    "River Rd",
];
const COUNTRY_CODES: &[&str] = &["VN", "US", "JP", "KR", "SG", "TH", "DE", "FR", "GB", "AU"];
const VI_COUNTRIES: &[&str] = &[
    "Việt Nam",
    "Hoa Kỳ",
    "Nhật Bản",
    "Hàn Quốc",
    "Singapore",
    "Thái Lan",
    "Đức",
    "Pháp",
    "Anh",
    "Úc",
];
const EN_COUNTRIES: &[&str] = &[
    "Vietnam",
    "United States",
    "Japan",
    "South Korea",
    "Singapore",
    "Thailand",
    "Germany",
    "France",
    "United Kingdom",
    "Australia",
];
const VI_COMPANY: &[&str] = &["Công ty TNHH", "Công ty Cổ phần", "Doanh nghiệp tư nhân"];
const VI_COMPANY_NAMES: &[&str] = &[
    "Sao Mai",
    "Hoà Phát",
    "Ánh Dương",
    "Thành Công",
    "Phương Nam",
    "Đại Việt",
    "Hưng Thịnh",
    "An Khang",
    "Bình Minh",
    "Tân Tiến",
];
const EN_COMPANY: &[&str] = &[
    "Acme",
    "Globex",
    "Initech",
    "Umbrella",
    "Stark",
    "Wayne",
    "Hooli",
    "Vandelay",
    "Wonka",
    "Cyberdyne",
];
const EN_COMPANY_SUFFIX: &[&str] = &["Inc.", "LLC", "Ltd.", "Group", "Corp."];
const VI_PRODUCTS: &[&str] = &[
    "Áo thun",
    "Quần jean",
    "Giày thể thao",
    "Bình nước",
    "Tai nghe",
    "Bàn phím",
    "Chuột không dây",
    "Sổ tay",
    "Ba lô",
    "Đèn bàn",
    "Cà phê rang xay",
    "Trà xanh",
];
const EN_PRODUCTS: &[&str] = &[
    "T-shirt",
    "Jeans",
    "Sneakers",
    "Water bottle",
    "Headphones",
    "Keyboard",
    "Wireless mouse",
    "Notebook",
    "Backpack",
    "Desk lamp",
    "Coffee beans",
    "Green tea",
];
const PRODUCT_ADJ_VI: &[&str] = &["cao cấp", "cơ bản", "mini", "Pro", "2026", "phiên bản giới hạn"];
const PRODUCT_ADJ_EN: &[&str] = &["Classic", "Pro", "Mini", "Deluxe", "2026", "Limited"];
const VI_STATUS: &[&str] = &["Mới", "Đang xử lý", "Hoàn thành", "Đã huỷ"];
const EN_STATUS: &[&str] = &["New", "Processing", "Completed", "Cancelled"];
const VI_WORDS: &[&str] = &[
    "khách", "hàng", "đơn", "giao", "nhanh", "chất", "lượng", "tốt", "hỗ", "trợ", "liên", "hệ", "ghi", "chú", "kiểm",
    "tra", "cập", "nhật", "thông", "tin", "dịch", "vụ", "sản", "phẩm", "mới",
];
const EN_WORDS: &[&str] = &[
    "customer", "order", "fast", "delivery", "quality", "support", "contact", "note", "check", "update", "service",
    "product", "new", "review", "request", "follow", "up", "please", "confirm", "details",
];
const VI_PHONE_PREFIX: &[&str] = &[
    "090", "091", "093", "094", "096", "097", "098", "032", "035", "038", "070", "077", "081", "085",
];

/// Vietnamese letters without diacritics (for e-mails and URLs).
fn fold(s: &str) -> String {
    const FROM: &str = "àáạảãâầấậẩẫăằắặẳẵèéẹẻẽêềếệểễìíịỉĩòóọỏõôồốộổỗơờớợởỡùúụủũưừứựửữỳýỵỷỹđ";
    const TO: &str = "aaaaaaaaaaaaaaaaaeeeeeeeeeeeiiiiiooooooooooooooooouuuuuuuuuuuyyyyyd";
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| {
            FROM.chars()
                .position(|f| f == c)
                .and_then(|i| TO.chars().nth(i))
                .unwrap_or(c)
        })
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .collect()
}

// ---------------------------------------------------------------------------
// choosing generators
// ---------------------------------------------------------------------------

fn table_kind(table: &str) -> &'static str {
    let t = fold(table);
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["product", "item", "sanpham", "mathang", "hanghoa", "goods", "article"]) {
        "product"
    } else if has(&[
        "compan",
        "supplier",
        "vendor",
        "congty",
        "nhacungcap",
        "partner",
        "doitac",
    ]) {
        "company"
    } else {
        "person"
    }
}

fn choose(table: &str, c: &Column, fk: bool, dialect: Dialect) -> (Generator, &'static str) {
    let ty = SqlType::parse(&c.data_type);
    let b = ty.base();
    // SQL Server's timestamp is rowversion, set by the server
    if c.is_identity || c.is_computed || b == "rowversion" || (dialect == Dialect::MsSql && b == "timestamp") {
        return (Generator::Skip, "filled by the server");
    }
    if fk {
        return (Generator::ForeignKey, "existing value from the referenced table");
    }
    let name = fold(&c.name).replace(' ', "");
    let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
    let ends = |words: &[&str]| words.iter().any(|w| name.ends_with(w));
    if c.is_primary_key {
        return match b {
            "uniqueidentifier" | "uuid" => (Generator::Guid, "new GUID"),
            _ if ty.is_integer() => (Generator::Sequence, "continues after the current maximum"),
            _ => (Generator::Code, "unique code"),
        };
    }
    match b {
        "uniqueidentifier" | "uuid" => return (Generator::Guid, "random GUID"),
        "bit" | "bool" | "boolean" => return (Generator::Boolean, "true / false"),
        "date" => {
            return if has(&["birth", "dob", "ngaysinh", "sinhnhat"]) {
                (Generator::BirthDate, "birth date (age 18–70)")
            } else {
                (Generator::Date, "date in the last 3 years")
            };
        }
        // Postgres spells these "timestamp with(out) time zone", "time without time zone"
        t if t.starts_with("datetime") || t.starts_with("timestamp") || t == "smalldatetime" => {
            return (Generator::DateTime, "date and time in the last 3 years");
        }
        t if t.starts_with("time") => return (Generator::DateTime, "time of day"),
        _ => {}
    }
    if ty.is_number() {
        if b == "tinyint" && has(&["is", "flag", "active"]) {
            return (Generator::Boolean, "0 / 1");
        }
        return if ty.is_decimal() {
            (Generator::Decimal, "amount")
        } else {
            (Generator::Integer, "number")
        };
    }
    if !ty.is_text() {
        return if c.nullable {
            (Generator::Null, "type not supported: NULL")
        } else {
            (Generator::Skip, "type not supported: left to its default")
        };
    }
    let len = ty.length().unwrap_or(4000);
    if has(&["email", "mail"]) {
        (Generator::Email, "e-mail address")
    } else if has(&["phone", "mobile", "tel", "sdt", "dienthoai", "fax"]) {
        (Generator::Phone, "phone number")
    } else if has(&["url", "website", "web", "link"]) {
        (Generator::Url, "web address")
    } else if has(&["address", "diachi", "street", "duong"]) {
        (Generator::Address, "street address")
    } else if has(&["city", "thanhpho", "tinh", "province", "town"]) {
        (Generator::City, "city")
    } else if has(&["country", "quocgia", "nation"]) {
        (Generator::Country, if len <= 3 { "country code" } else { "country" })
    } else if has(&["gender", "gioitinh", "sex", "phai"]) {
        (Generator::Gender, "gender")
    } else if has(&["status", "trangthai", "state", "tinhtrang"]) {
        (Generator::Status, "status")
    } else if has(&["firstname", "givenname", "tendem"]) || name == "ten" {
        (Generator::FirstName, "first name")
    } else if has(&["lastname", "surname", "familyname"]) || name == "ho" {
        (Generator::LastName, "last name")
    } else if has(&["company", "congty", "supplier", "vendor", "nhacungcap"]) {
        (Generator::Company, "company name")
    } else if has(&["product", "sanpham", "item", "mathang"]) && has(&["name", "ten"]) {
        (Generator::Product, "product name")
    } else if has(&[
        "fullname",
        "hoten",
        "hovaten",
        "customername",
        "tenkhachhang",
        "tenkh",
        "tensinhvien",
        "tennhanvien",
    ]) || ends(&["name", "ten"])
    {
        match table_kind(table) {
            "product" => (Generator::Product, "product name"),
            "company" => (Generator::Company, "company name"),
            _ => (Generator::FullName, "full name"),
        }
    } else if has(&["code", "ma", "sku", "number", "no", "so"]) && len <= 30 {
        (Generator::Code, "code")
    } else {
        (Generator::Text, "words")
    }
}

/// Generators for every column of `table`.
pub fn plan_columns(table: &DbObject, foreign_keys: &[ForeignKey], dialect: Dialect) -> Vec<ColumnPlan> {
    table
        .columns
        .iter()
        .map(|c| {
            let fk = foreign_keys.iter().any(|f| {
                f.from_table.eq_ignore_ascii_case(&table.name)
                    && f.from_schema.eq_ignore_ascii_case(&table.schema)
                    && f.from_columns.iter().any(|x| x.eq_ignore_ascii_case(&c.name))
            });
            let (generator, note) = choose(&table.name, c, fk, dialect);
            ColumnPlan {
                column: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: c.nullable,
                generator,
                note: note.into(),
            }
        })
        .collect()
}

/// Foreign keys of `table` whose columns use the ForeignKey generator.
pub fn foreign_keys_of<'a>(table: &DbObject, foreign_keys: &'a [ForeignKey]) -> Vec<&'a ForeignKey> {
    foreign_keys
        .iter()
        .filter(|f| f.from_table.eq_ignore_ascii_case(&table.name) && f.from_schema.eq_ignore_ascii_case(&table.schema))
        .collect()
}

// ---------------------------------------------------------------------------
// values
// ---------------------------------------------------------------------------

/// Days since 1970-01-01 → (y, m, d) (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// 2026-10-20 as days since the epoch; dates are generated before it so
/// scripts are reproducible.
const TODAY: i64 = 20_746;

enum Value {
    Null,
    Raw(String),
    Text(String),
}

fn fit(s: String, len: Option<usize>) -> String {
    match len {
        Some(n) if s.chars().count() > n => s.chars().take(n).collect(),
        _ => s,
    }
}

fn person(rng: &mut Rng, locale: Locale) -> (String, String, String) {
    match locale {
        Locale::Vi => (
            rng.pick(VI_LAST).into(),
            format!("{} {}", rng.pick(VI_MIDDLE), rng.pick(VI_FIRST)),
            String::new(),
        ),
        Locale::En => (rng.pick(EN_LAST).into(), rng.pick(EN_FIRST).into(), String::new()),
    }
}

fn full_name(rng: &mut Rng, locale: Locale) -> String {
    let (last, first, _) = person(rng, locale);
    match locale {
        Locale::Vi => format!("{last} {first}"),
        Locale::En => format!("{first} {last}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn value(
    g: Generator,
    c: &ColumnPlan,
    row: usize,
    rng: &mut Rng,
    opts: &TestDataOptions,
    ctx: &DataContext,
    fk_row: Option<&HashMap<String, Option<String>>>,
    dialect: Dialect,
) -> Value {
    let loc = opts.locale;
    let ty = SqlType::parse(&c.data_type);
    let len = ty.length();
    let text = |s: String| Value::Text(fit(s, len));
    match g {
        Generator::Skip | Generator::Null => Value::Null,
        Generator::Fixed => match opts.fixed.get(&c.column) {
            Some(v) if v.eq_ignore_ascii_case("null") => Value::Null,
            Some(v) if ty.is_number() => Value::Raw(v.clone()),
            Some(v) => text(v.clone()),
            None => Value::Null,
        },
        Generator::FullName => text(full_name(rng, loc)),
        Generator::FirstName => {
            let (_, first, _) = person(rng, loc);
            text(first)
        }
        Generator::LastName => {
            let (last, _, _) = person(rng, loc);
            text(last)
        }
        Generator::Email => {
            let name = fold(&full_name(rng, loc));
            let parts: Vec<&str> = name.split_whitespace().collect();
            let local = match (parts.last(), parts.first()) {
                (Some(l), Some(f)) if loc == Locale::Vi => format!("{l}.{f}"),
                (Some(l), Some(f)) => format!("{f}.{l}"),
                _ => "user".into(),
            };
            // reserved domains only (RFC 2606 / 6761), so no address can reach a real mailbox
            let domain = ["example.com", "example.net", "mail.test", "demo.local"][rng.below(4)];
            text(format!("{local}{}@{domain}", row + 1))
        }
        Generator::Phone => text(match loc {
            Locale::Vi => format!("{}{:07}", rng.pick(VI_PHONE_PREFIX), rng.range(0, 9_999_999)),
            Locale::En => format!("+1-555-{:03}-{:04}", rng.range(100, 999), rng.range(0, 9999)),
        }),
        Generator::Address => text(match loc {
            Locale::Vi => format!(
                "{} {}, {}",
                rng.range(1, 400),
                rng.pick(VI_STREETS),
                rng.pick(VI_CITIES)
            ),
            Locale::En => format!(
                "{} {}, {}",
                rng.range(1, 9999),
                rng.pick(EN_STREETS),
                rng.pick(EN_CITIES)
            ),
        }),
        Generator::City => text(rng.pick(if loc == Locale::Vi { VI_CITIES } else { EN_CITIES }).into()),
        Generator::Country => {
            let i = rng.below(COUNTRY_CODES.len());
            text(if len.is_some_and(|n| n <= 3) {
                COUNTRY_CODES[i].into()
            } else if loc == Locale::Vi {
                VI_COUNTRIES[i].into()
            } else {
                EN_COUNTRIES[i].into()
            })
        }
        Generator::Company => text(match loc {
            Locale::Vi => format!("{} {}", rng.pick(VI_COMPANY), rng.pick(VI_COMPANY_NAMES)),
            Locale::En => format!("{} {}", rng.pick(EN_COMPANY), rng.pick(EN_COMPANY_SUFFIX)),
        }),
        Generator::Product => text(match loc {
            Locale::Vi => format!("{} {}", rng.pick(VI_PRODUCTS), rng.pick(PRODUCT_ADJ_VI)),
            Locale::En => format!("{} {}", rng.pick(PRODUCT_ADJ_EN), rng.pick(EN_PRODUCTS)),
        }),
        Generator::Code => text(code(&c.column, row, opts.rows, len)),
        Generator::Status => text(rng.pick(if loc == Locale::Vi { VI_STATUS } else { EN_STATUS }).into()),
        Generator::Gender => {
            if len == Some(1) {
                text(rng.pick(&["M", "F"]).into())
            } else {
                text(
                    rng.pick(if loc == Locale::Vi {
                        &["Nam", "Nữ"]
                    } else {
                        &["Male", "Female"]
                    })
                    .into(),
                )
            }
        }
        Generator::Url => {
            let name = fold(&full_name(rng, loc)).replace(' ', "");
            text(format!("https://{name}{}.example.com", row + 1))
        }
        Generator::Text => {
            let words = if loc == Locale::Vi { VI_WORDS } else { EN_WORDS };
            let n = rng.range(2, 8) as usize;
            let sentence = (0..n).map(|_| rng.pick(words)).collect::<Vec<_>>().join(" ");
            let mut chars = sentence.chars();
            let capitalized: String = chars
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .chain(chars)
                .collect();
            text(capitalized)
        }
        Generator::Integer => {
            let name = fold(&c.column);
            let (lo, hi) = if ["qty", "quantity", "soluong", "count"].iter().any(|w| name.contains(w)) {
                (1, 100)
            } else if ["age", "tuoi"].iter().any(|w| name.contains(w)) {
                (18, 80)
            } else if ["year", "nam"].iter().any(|w| name.ends_with(w)) {
                (2000, 2026)
            } else {
                match ty.base() {
                    // MySQL's tinyint is signed (-128..127) unless declared unsigned
                    "tinyint" if dialect == Dialect::MySql && !ty.is_unsigned() => (0, 127),
                    "tinyint" => (0, 255),
                    "smallint" | "int2" => (1, 10_000),
                    _ => (1, 100_000),
                }
            };
            Value::Raw(rng.range(lo, hi).to_string())
        }
        Generator::Decimal => {
            let money = matches!(ty.base(), "money" | "smallmoney");
            let s = ty.scale().unwrap_or(if money { 2 } else { 0 }).min(4);
            // decimal(5, 2) holds up to 999.99: stay below 10^(precision - scale)
            let top: i64 = match ty.integer_digits() {
                Some(d) if d < 7 => 10i64.pow(d) * 100 - 1,
                _ => 1_000_000,
            };
            let cents = rng.range(1.min(top), top.max(1));
            let v = cents as f64 / 100.0;
            Value::Raw(format!("{v:.prec$}", prec = s as usize))
        }
        Generator::Date | Generator::BirthDate | Generator::DateTime => {
            let days = match g {
                Generator::BirthDate => TODAY - rng.range(18 * 365, 70 * 365),
                _ => TODAY - rng.range(0, 3 * 365),
            };
            let (y, m, d) = civil(days);
            let date = format!("{y:04}-{m:02}-{d:02}");
            let time = format!(
                "{:02}:{:02}:{:02}",
                rng.range(7, 21),
                rng.range(0, 59),
                rng.range(0, 59)
            );
            if g == Generator::DateTime && ty.base().starts_with("time") && !ty.base().starts_with("timestamp") {
                Value::Text(time)
            } else if g == Generator::DateTime {
                Value::Text(format!("{date} {time}"))
            } else {
                Value::Text(date)
            }
        }
        // PostgreSQL bit(n) is a string of n bits
        Generator::Boolean if dialect == Dialect::Postgres && matches!(ty.base(), "bit" | "varbit" | "bit varying") => {
            let n = len.unwrap_or(1).clamp(1, 64);
            Value::Raw(format!(
                "B'{}'",
                (0..n)
                    .map(|_| if rng.percent(50) { '1' } else { '0' })
                    .collect::<String>()
            ))
        }
        Generator::Boolean => Value::Raw(if rng.percent(50) { "1" } else { "0" }.into()),
        Generator::Guid => {
            let a = rng.next();
            let b = rng.next();
            let hex = format!("{a:016x}{b:016x}");
            Value::Text(format!(
                "{}-{}-4{}-{}{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[13..16],
                ["8", "9", "a", "b"][rng.below(4)],
                &hex[17..20],
                &hex[20..32]
            ))
        }
        Generator::Sequence => {
            let start = ctx.max_values.get(&c.column.to_lowercase()).copied().unwrap_or(0);
            Value::Raw((start + row as i64 + 1).to_string())
        }
        Generator::ForeignKey => match fk_row.and_then(|r| r.get(&c.column.to_lowercase())) {
            Some(Some(v)) if ty.is_number() => Value::Raw(v.clone()),
            Some(Some(v)) => Value::Text(v.clone()),
            _ => Value::Null,
        },
    }
}

/// A code unique per row: `PREFIX0001` when it fits the column, otherwise
/// the row number in base 36 over the whole width (`char(2)` holds 1,296).
fn code(column: &str, row: usize, rows: usize, len: Option<usize>) -> String {
    let prefix: String = fold(column)
        .chars()
        .filter(char::is_ascii_alphabetic)
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let prefix = if prefix.is_empty() { "C".to_string() } else { prefix };
    let digits = rows.max(1).to_string().len().max(4);
    let width = len.unwrap_or(10);
    if prefix.len() + digits <= width {
        return format!("{prefix}{:0digits$}", row + 1);
    }
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut n = row;
    let mut out = vec![b'0'; width.max(1)];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[n % 36];
        n /= 36;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// How many different codes fit in a column of `len` characters.
fn code_capacity(len: Option<usize>) -> usize {
    match len {
        Some(n) if n < 6 => 36usize.pow(n as u32),
        _ => usize::MAX,
    }
}

fn literal(v: Value, c: &ColumnPlan, dialect: Dialect) -> String {
    let ty = SqlType::parse(&c.data_type);
    match v {
        Value::Null => "NULL".into(),
        Value::Raw(s) if matches!(ty.base(), "bool" | "boolean") => if s == "1" { "TRUE" } else { "FALSE" }.into(),
        Value::Raw(s) => s,
        Value::Text(s) => dialect.typed_string(&s, &ty),
    }
}

/// INSERT statements with `opts.rows` generated rows (1,000 rows per
/// statement, the SQL Server limit for VALUES lists).
pub fn insert_script(
    table: &DbObject,
    plans: &[ColumnPlan],
    foreign_keys: &[ForeignKey],
    ctx: &DataContext,
    opts: &TestDataOptions,
    dialect: Dialect,
) -> Result<String, String> {
    let gen_of = |p: &ColumnPlan| opts.overrides.get(&p.column).copied().unwrap_or(p.generator);
    let cols: Vec<&ColumnPlan> = plans.iter().filter(|p| gen_of(p) != Generator::Skip).collect();
    if cols.is_empty() {
        return Err("Every column is skipped: nothing to insert.".into());
    }
    // a parent row per foreign key per row, so composite keys stay consistent
    let fks: Vec<&ForeignKey> = foreign_keys_of(table, foreign_keys)
        .into_iter()
        .filter(|f| {
            f.from_columns.iter().any(|fc| {
                cols.iter()
                    .any(|p| p.column.eq_ignore_ascii_case(fc) && gen_of(p) == Generator::ForeignKey)
            })
        })
        .collect();
    for f in &fks {
        let empty = ctx.parent_rows.get(&f.name).is_none_or(|r| r.is_empty());
        let required = f
            .from_columns
            .iter()
            .any(|fc| plans.iter().any(|p| p.column.eq_ignore_ascii_case(fc) && !p.nullable));
        if empty && required {
            return Err(format!(
                "{}.{} has no rows, so {} cannot reference it. Generate data for {} first, or set the column to a fixed value.",
                f.to_schema,
                f.to_table,
                f.from_columns.join(", "),
                f.to_table
            ));
        }
    }
    for p in cols.iter().filter(|p| gen_of(p) == Generator::Code) {
        let capacity = code_capacity(SqlType::parse(&p.data_type).length());
        if opts.rows > capacity {
            return Err(format!(
                "{} ({}) can hold only {capacity} different codes; generate fewer rows or choose another generator for it.",
                p.column, p.data_type
            ));
        }
    }
    let mut rng = Rng::new(opts.seed);
    let quote = |n: &str| dialect.quote_ident(n);
    let target = object_name(table, dialect, true);
    let header = cols.iter().map(|p| quote(&p.column)).collect::<Vec<_>>().join(", ");
    let rows = opts.rows.max(1);
    let mut out = format!(
        "-- Test data for {}: {rows} row(s), generated by Sqlotus\n-- Columns: {}\n",
        one_line(&target),
        one_line(
            &cols
                .iter()
                .map(|p| format!("{} = {}", p.column, format!("{:?}", gen_of(p)).to_lowercase()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    );
    let skipped: Vec<&str> = plans
        .iter()
        .filter(|p| gen_of(p) == Generator::Skip)
        .map(|p| p.column.as_str())
        .collect();
    if !skipped.is_empty() {
        out.push_str(&format!("-- Not inserted: {}\n", one_line(&skipped.join(", "))));
    }
    let mut lines = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut fk_row: HashMap<String, Option<String>> = HashMap::new();
        for f in &fks {
            if let Some(parents) = ctx.parent_rows.get(&f.name).filter(|r| !r.is_empty()) {
                let pick = &parents[rng.below(parents.len())];
                for (i, fc) in f.from_columns.iter().enumerate() {
                    fk_row.insert(fc.to_lowercase(), pick.get(i).cloned().flatten());
                }
            }
        }
        let values: Vec<String> = cols
            .iter()
            .map(|p| {
                let g = gen_of(p);
                let null = p.nullable
                    && !matches!(g, Generator::Fixed | Generator::Sequence | Generator::Null)
                    && rng.percent(opts.null_percent.min(100));
                let v = if null {
                    Value::Null
                } else {
                    value(g, p, row, &mut rng, opts, ctx, Some(&fk_row), dialect)
                };
                literal(v, p, dialect)
            })
            .collect();
        lines.push(format!("({})", values.join(", ")));
    }
    for chunk in lines.chunks(1000) {
        out.push_str(&format!(
            "INSERT INTO {target} ({header})\nVALUES {};\n",
            chunk.join(",\n       ")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ObjectKind;

    fn col(name: &str, ty: &str, nullable: bool, pk: bool, identity: bool) -> Column {
        Column {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            is_primary_key: pk,
            is_identity: identity,
            is_computed: false,
        }
    }

    fn customers() -> DbObject {
        DbObject {
            schema: "dbo".into(),
            name: "KhachHang".into(),
            kind: ObjectKind::Table,
            columns: vec![
                col("Id", "int", false, true, true),
                col("HoTen", "nvarchar(100)", false, false, false),
                col("Email", "varchar(100)", true, false, false),
                col("SoDienThoai", "varchar(15)", true, false, false),
                col("NgaySinh", "date", true, false, false),
                col("QuocGia", "char(2)", true, false, false),
                col("DiaChi", "nvarchar(200)", true, false, false),
                col("NgayTao", "datetime2(7)", false, false, false),
                col("ConHoatDong", "bit", false, false, false),
            ],
            params: vec![],
        }
    }

    fn orders() -> (DbObject, ForeignKey) {
        let t = DbObject {
            schema: "dbo".into(),
            name: "Orders".into(),
            kind: ObjectKind::Table,
            columns: vec![
                col("OrderNo", "int", false, true, false),
                col("CustomerId", "int", false, false, false),
                col("Total", "decimal(12, 2)", false, false, false),
                col("Status", "nvarchar(20)", true, false, false),
                col("RowVer", "rowversion", false, false, false),
            ],
            params: vec![],
        };
        let fk = ForeignKey {
            name: "FK_Orders_Customers".into(),
            from_schema: "dbo".into(),
            from_table: "Orders".into(),
            from_columns: vec!["CustomerId".into()],
            to_schema: "dbo".into(),
            to_table: "KhachHang".into(),
            to_columns: vec!["Id".into()],
        };
        (t, fk)
    }

    #[test]
    fn picks_generators_from_names_and_types() {
        let plans = plan_columns(&customers(), &[], Dialect::MsSql);
        let g: Vec<Generator> = plans.iter().map(|p| p.generator).collect();
        use Generator::*;
        assert_eq!(
            g,
            vec![
                Skip, FullName, Email, Phone, BirthDate, Country, Address, DateTime, Boolean
            ]
        );
        let (o, fk) = orders();
        let g: Vec<Generator> = plan_columns(&o, &[fk], Dialect::MsSql)
            .iter()
            .map(|p| p.generator)
            .collect();
        assert_eq!(g, vec![Sequence, ForeignKey, Decimal, Status, Skip]);
        let pg = DbObject {
            columns: vec![
                col("created_at", "timestamp with time zone", false, false, false),
                col("opens", "time without time zone", true, false, false),
                col("doc", "bytea", false, false, false),
            ],
            ..customers()
        };
        let g: Vec<Generator> = plan_columns(&pg, &[], Dialect::Postgres)
            .iter()
            .map(|p| p.generator)
            .collect();
        assert_eq!(g, vec![DateTime, DateTime, Skip]);
        let ts = DbObject {
            columns: vec![col("CreatedAt", "timestamp", false, false, false)],
            ..customers()
        };
        assert_eq!(plan_columns(&ts, &[], Dialect::MsSql)[0].generator, Skip);
        assert_eq!(plan_columns(&ts, &[], Dialect::Postgres)[0].generator, DateTime);
    }

    #[test]
    fn mysql_unsigned_columns_are_numbers() {
        // MySQL 8 writes `int unsigned` without a display width
        let t = DbObject {
            schema: String::new(),
            name: "stock".into(),
            kind: ObjectKind::Table,
            columns: vec![
                col("id", "int unsigned", false, true, false),
                col("qty", "smallint unsigned", false, false, false),
                col("level", "tinyint unsigned", true, false, false),
            ],
            params: vec![],
        };
        let g: Vec<Generator> = plan_columns(&t, &[], Dialect::MySql)
            .iter()
            .map(|p| p.generator)
            .collect();
        assert_eq!(g, vec![Generator::Sequence, Generator::Integer, Generator::Integer]);
    }

    #[test]
    fn generates_valid_rows() {
        let t = customers();
        let plans = plan_columns(&t, &[], Dialect::MsSql);
        let opts = TestDataOptions {
            rows: 1500,
            locale: Locale::Vi,
            null_percent: 0,
            seed: 7,
            ..Default::default()
        };
        let s = insert_script(&t, &plans, &[], &DataContext::default(), &opts, Dialect::MsSql).unwrap();
        assert_eq!(s.matches("INSERT INTO dbo.KhachHang").count(), 2, "split at 1000 rows");
        assert!(s.contains("(HoTen, Email, SoDienThoai, NgaySinh, QuocGia, DiaChi, NgayTao, ConHoatDong)"));
        assert!(
            s.contains("N'Nguy") || s.contains("N'Tr") || s.contains("N'L"),
            "Vietnamese names as nvarchar"
        );
        // deterministic
        assert_eq!(
            s,
            insert_script(&t, &plans, &[], &DataContext::default(), &opts, Dialect::MsSql).unwrap()
        );
        let first = s.lines().find(|l| l.starts_with("VALUES")).unwrap();
        assert!(
            first.contains("@example") || first.contains("@mail") || first.contains("@demo"),
            "{first}"
        );
        // lengths are respected: char(2) country codes
        assert!(
            s.lines()
                .filter(|l| l.contains("VALUES") || l.trim_start().starts_with('('))
                .all(|l| !l.contains("'Việt Nam'"))
        );
    }

    #[test]
    fn foreign_keys_and_sequences_use_database_values() {
        let (o, fk) = orders();
        let plans = plan_columns(&o, std::slice::from_ref(&fk), Dialect::MsSql);
        let mut ctx = DataContext::default();
        ctx.parent_rows
            .insert(fk.name.clone(), vec![vec![Some("7".into())], vec![Some("9".into())]]);
        ctx.max_values.insert("orderno".into(), 41);
        let opts = TestDataOptions {
            rows: 3,
            seed: 1,
            ..Default::default()
        };
        let s = insert_script(&o, &plans, std::slice::from_ref(&fk), &ctx, &opts, Dialect::MsSql).unwrap();
        let rows: Vec<&str> = s
            .lines()
            .filter(|l| l.contains('(') && !l.starts_with("--") && !l.starts_with("INSERT"))
            .collect();
        assert!(rows[0].contains("(42, ") && rows[2].contains("(44, "), "{rows:?}");
        assert!(
            rows.iter().all(|r| r.contains(", 7, ") || r.contains(", 9, ")),
            "{rows:?}"
        );
        assert!(s.contains("(OrderNo, CustomerId, Total, Status)") && s.contains("-- Not inserted: RowVer"));
        // no parent rows: explain instead of producing invalid data
        let err = insert_script(
            &o,
            &plans,
            std::slice::from_ref(&fk),
            &DataContext::default(),
            &opts,
            Dialect::MsSql,
        )
        .unwrap_err();
        assert!(err.contains("has no rows"), "{err}");
    }

    #[test]
    fn overrides_fixed_values_and_dialects() {
        let t = customers();
        let plans = plan_columns(&t, &[], Dialect::Postgres);
        let mut opts = TestDataOptions {
            rows: 2,
            locale: Locale::En,
            ..Default::default()
        };
        opts.overrides.insert("HoTen".into(), Generator::Fixed);
        opts.fixed.insert("HoTen".into(), "O'Neil".into());
        opts.overrides.insert("Email".into(), Generator::Skip);
        let s = insert_script(&t, &plans, &[], &DataContext::default(), &opts, Dialect::Postgres).unwrap();
        assert!(s.contains("'O''Neil'") && !s.contains("N'O"));
        assert!(!s.contains("\"Email\"") && !s.contains(" Email,"));
        // a PostgreSQL bit column is a bit string, a boolean column TRUE / FALSE
        assert!(s.contains("B'0'") || s.contains("B'1'"), "{s}");
        let mut b = t.clone();
        b.columns
            .iter_mut()
            .find(|c| c.name == "ConHoatDong")
            .unwrap()
            .data_type = "boolean".into();
        let bp = plan_columns(&b, &[], Dialect::Postgres);
        let s = insert_script(&b, &bp, &[], &DataContext::default(), &opts, Dialect::Postgres).unwrap();
        assert!(s.contains("TRUE") || s.contains("FALSE"), "{s}");
        assert_eq!(fold("Nguyễn Văn Đức"), "nguyen van duc");
        // short codes stay unique: base 36 over the whole width
        let codes: std::collections::HashSet<String> = (0..1296).map(|r| code("Code", r, 1296, Some(2))).collect();
        assert_eq!(codes.len(), 1296);
        assert!(codes.iter().all(|c| c.len() == 2));
        assert_eq!(code("Sku", 0, 100, Some(20)), "SK0001");
        let short = DbObject {
            columns: vec![col("Code", "char(2)", false, true, false)],
            ..customers()
        };
        let plans = plan_columns(&short, &[], Dialect::MsSql);
        let too_many = TestDataOptions {
            rows: 2000,
            ..Default::default()
        };
        assert!(
            insert_script(&short, &plans, &[], &DataContext::default(), &too_many, Dialect::MsSql)
                .unwrap_err()
                .contains("1296")
        );
        assert_eq!(civil(TODAY), (2026, 10, 20));
    }
}
