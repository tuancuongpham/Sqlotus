<div align="center">

[English](../../README.md) · **Tiếng Việt**

<img src="../../src-tauri/icons/128x128@2x.png" alt="Icon Sqlotus: bông sen trên khối database" width="104">

# Sqlotus

### Trình soạn SQL desktop nhanh cho SQL Server, PostgreSQL và MySQL.

**Viết query nhanh hơn, bắt lỗi trước khi chạy và hiểu rõ database — mọi thứ nằm trên máy của bạn.**

[![Latest release](https://img.shields.io/github/v/release/tuancuongpham/Sqlotus?style=flat-square&color=2f81f7)](https://github.com/tuancuongpham/Sqlotus/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/tuancuongpham/Sqlotus/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/tuancuongpham/Sqlotus/actions/workflows/ci.yml)
[![Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078d4?style=flat-square)](#bắt-đầu-nhanh)
[![Databases](https://img.shields.io/badge/SQL%20Server%20%7C%20PostgreSQL%20%7C%20MySQL-supported-cc2927?style=flat-square)](#làm-được-gì)
[![Built with Rust and Tauri](https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri-dea584?style=flat-square&logo=rust&logoColor=white)](#sqlotus-hoạt-động-thế-nào)
[![MIT license](https://img.shields.io/badge/license-MIT-f4c430?style=flat-square)](../../LICENSE)

<img src="../images/completion.png" alt="Sqlotus gợi ý JOIN kèm điều kiện ON từ khoá ngoại, phía trên lưới kết quả" width="900">

</div>

## Bắt đầu nhanh

### Cài đặt

1. Tải **`Sqlotus_<version>_x64-setup.exe`** từ [bản phát hành mới nhất](https://github.com/tuancuongpham/Sqlotus/releases/latest)
   (file `.msi` dành cho cài đặt tập trung cho cả máy).
2. Chạy file cài — cài cho người dùng hiện tại, không cần quyền Administrator. Windows 11 có sẵn WebView2;
   trên Windows 10 bộ cài tự cài nếu thiếu.
3. Bộ cài chưa được ký số nên SmartScreen có thể cảnh báo: chọn **More info → Run anyway**. File
   `SHA256SUMS.txt` trong bản phát hành dùng để kiểm tra file tải về.

### Kết nối

Bấm nút kết nối ở góc trên bên trái, chọn **SQL Server**, **PostgreSQL** hoặc **MySQL**, nhập host, port và tài khoản
(SQL Server có thêm Windows authentication). Nếu kết nối lỗi, **Connection Doctor** giải thích nguyên nhân và đề xuất
cách sửa.

### Thử ngay

| Gõ hoặc nhấn | Kết quả |
|---|---|
| `SELECT * FROM dbo.Orders o JOIN ` | gợi ý các bảng liên quan kèm sẵn điều kiện `ON` |
| `SELECT *` rồi **Tab** | `*` được thay bằng danh sách cột |
| **Ctrl+K, Ctrl+Y** | format script |
| **Ctrl+L** | kế hoạch thực thi ước tính, không chạy query |
| `UPDATE … ` không có `WHERE`, rồi **F5** | Sqlotus hỏi lại trước khi chạy |

### Cập nhật

Chạy bộ cài bản mới. Kết nối, mật khẩu đã lưu, lịch sử và cài đặt được giữ nguyên.

## Làm được gì

| Nhóm | Nổi bật |
|---|---|
| **Viết SQL nhanh** | gợi ý theo ngữ cảnh, JOIN theo khoá ngoại, sinh `INSERT`/`EXEC`, mở rộng `*`, snippet |
| **Format và refactor** | formatter tuỳ chỉnh, qualify tên, đổi tên biến và alias, đi tới định nghĩa |
| **An toàn** | phân tích khi gõ, hỏi lại câu lệnh nguy hiểm, kết nối read-only và production, nhắc transaction đang mở |
| **Hiểu database** | Object Search, Object Explorer, sơ đồ ER, Used by, sức khoẻ index, Activity, so sánh schema |
| **Làm việc với kết quả** | lưới ảo hoá, sửa dữ liệu, copy dạng INSERT/CSV/JSON/Markdown, Excel, biểu đồ, so sánh kết quả, kế hoạch thực thi |
| **Không mất query** | tự lưu phiên bản mọi tab, tìm full-text, mở lại tab đã đóng, khôi phục tab khi mở lại app |

Bấm vào từng nhóm để xem chi tiết.

<details>
<summary><b>Viết SQL nhanh</b></summary>

- **Gợi ý theo ngữ cảnh**: keyword, schema/bảng/view/cột/proc/function; hiểu alias, CTE, derived table, `#temp`,
  `@table`; tự gán alias (`OrderDetails` → `od`); biến `@x`.
- **JOIN theo khoá ngoại**: sau `JOIN` gợi ý bảng liên quan kèm `ON`, sau `ON` gợi ý điều kiện join.
- **Sinh câu lệnh**: chọn bảng sau `INSERT INTO` → danh sách cột + `VALUES` có chú thích kiểu (Tab nhảy giữa các giá
  trị); `EXEC proc` → mỗi tham số một dòng kèm kiểu; `GROUP BY` → các cột không phải aggregate.
- **Mở rộng `*`**: gõ `SELECT *` (hoặc `t.*`) rồi nhấn **Tab** để thay bằng danh sách cột.
- **Gợi ý xuyên database**: `OtherDb.` → schema, `OtherDb.dbo.` → bảng, và cột của bảng ở database khác.
- **Hover** xem cột của bảng, kiểu của cột hoặc biến; **gợi ý tham số** khi gõ hàm/proc.
- **Snippet** có placeholder; import thêm từ một thư mục chứa các file snippet `.json` (Options → Snippets).

</details>

<details>
<summary><b>Format và refactor</b></summary>

- **Format SQL** (Ctrl+K, Ctrl+Y): chữ hoa/thường keyword, danh sách căn lề hoặc xếp chồng, dấu phẩy đầu/cuối, thụt
  JOIN, ON/AND/OR xuống dòng. Formatter tự kiểm tra chỉ khoảng trắng và kiểu chữ keyword thay đổi.
- **Thao tác** (chuột phải hoặc F1): Expand wildcards, Qualify object names, Insert semicolons, Remove square
  brackets, Uppercase/Lowercase keywords — mỗi thao tác hoàn tác được bằng một lần Undo.
- **Đổi tên** (F2) biến, alias, CTE ở mọi chỗ dùng.
- **Đi tới định nghĩa** (F12) và **Script as ALTER** (Alt+F12).

</details>

<details>
<summary><b>An toàn và phân tích code</b></summary>

- **Gạch chân khi gõ**: UPDATE/DELETE thiếu WHERE, `SELECT *`, `= NULL`, kiểu deprecated, biến không dùng,
  object/cột không tồn tại, chuỗi tiếng Việt thiếu tiền tố `N` (`'Nguyễn'` sẽ thành `Nguy?n`),
  `NOT IN (SELECT …)` trên cột cho phép NULL, hàm bọc cột trong WHERE/ON (`YEAR(NgayTao) = 2024` không dùng được
  index), so sánh gây chuyển kiểu ngầm (`cot_varchar = N'x'`, `cot_chuoi = 123`).
- **Cảnh báo trước khi chạy** UPDATE/DELETE không WHERE, TRUNCATE, DROP.
- **Kết nối read-only**: chặn INSERT/UPDATE/DELETE/MERGE/DDL/GRANT, vẫn chạy được SELECT (tab có 🔒).
- **Nhắc transaction đang mở**: tab hiện **TRAN**, thanh công cụ có nút Commit / Roll back, đóng tab sẽ hỏi.
- **Hỏi trước khi lưu thay đổi lớn**: script chỉ INSERT/UPDATE/DELETE chạy trong transaction; nếu đổi quá số dòng
  giới hạn (mặc định 1.000, Options → Safety) hoặc lỗi giữa chừng thì hỏi Commit hay Roll back; ít hơn thì tự commit
  và báo số dòng. **Manual commit** theo từng kết nối: thay đổi nằm trong transaction tới khi bấm Commit.
- **Môi trường kết nối** Development/Test/Staging/Production: Production có tab đỏ, nhãn PROD, manual commit và hỏi
  lại (ghi rõ server) trước mọi thay đổi.
- **Biến chưa khai báo**: chạy `… WHERE Id = @Id` mà chưa `DECLARE` sẽ hỏi giá trị (và kiểu), tự khai báo, nhớ giá trị.
- **Find invalid objects**: view/proc/function/trigger tham chiếu tới bảng/cột không còn tồn tại.

</details>

<details>
<summary><b>Connection Doctor và thông báo</b></summary>

- Khi kết nối lỗi, **Connection Doctor** giải thích nguyên nhân: server từ chối, timeout, sai host/instance, sai đăng
  nhập, chứng chỉ, database, Windows Auth. Với SQL Server trên máy này, nó kiểm tra service, TCP/IP, port, SQL Server
  Browser và chế độ xác thực.
- **Sửa một chạm**: bật TCP/IP, khởi động service, bật SQL Server Browser, bật Mixed Mode (Windows hỏi quyền
  Administrator), dùng đúng port/host, chuyển Windows Auth, tin chứng chỉ, chọn database khác, mở port trên Windows
  Firewall, bật lại login bị disable; sửa xong tự kết nối lại.
- Lỗi **18456** trên máy này: đọc lý do thật từ Windows event log (sai mật khẩu, login không tồn tại, bị disable, hết
  hạn…). Nút **Check server** kiểm tra ngay cả khi chưa có lỗi, ví dụ để máy khác kết nối được.
- **Thông báo cho mọi thao tác và cài đặt**: kết nối, chạy xong (thời gian, số dòng), lỗi kèm nút Diagnose/Retry,
  format, copy, lưu CSV, lưu kết nối và cài đặt, khôi phục tab.

</details>

<details>
<summary><b>Tìm kiếm, lịch sử, explorer</b></summary>

- **Object Search** (Ctrl+Alt+D): tìm theo tên object, tên cột, nội dung view/proc/function/trigger trong một hoặc tất
  cả database; xem định nghĩa có highlight, mở ra tab mới.
- **Query History** (Ctrl+Shift+H): tự lưu phiên bản các tab (định kỳ và mỗi lần Execute), tìm full-text, mở lại tab đã
  đóng (Ctrl+Shift+T), diff với phiên bản trước hoặc với tab hiện tại. Các tab đang mở được khôi phục khi mở lại app.
- **Object Explorer** (F8, mở sẵn): lọc object/cột, Select top 100, script SELECT/INSERT/UPDATE/DELETE/EXEC/CREATE/ALTER.
- **Sơ đồ ER**: bảng và khoá ngoại của cả database hoặc một bảng cùng các bảng liên quan, tự sắp xếp; kéo thả, zoom,
  tô sáng quan hệ, copy ảnh hoặc lưu SVG.
- **Used by**: view, proc, function, trigger và khoá ngoại nào đang dùng bảng/cột/module, kèm dòng — xem trước khi đổi
  tên hoặc xoá.
- **So sánh schema** (thanh công cụ → Compare): hai database trên cùng hoặc khác kết nối — bảng (cột, kiểu, NULL,
  default, identity, computed), khoá chính, index, unique, khoá ngoại, check, view, procedure, function, trigger; xem
  khác biệt từng object song song. Script đồng bộ sắp theo phụ thuộc (bỏ khoá ngoại trước, tạo lại sau cùng, tạo schema
  còn thiếu), ghi cảnh báo mất dữ liệu ở đầu, mở ra tab đã kết nối tới database đích và không tự chạy. Object chỉ có ở
  đích không được chọn sẵn. SQL Server cần quyền VIEW DEFINITION; thiếu quyền thì từ chối so sánh vì object bị ẩn sẽ
  trông như không tồn tại.
- **Sức khoẻ index**: index server gợi ý còn thiếu, index trùng/chồng lấn, index không ai đọc, bảng bị đọc toàn bộ,
  heap lớn. Cách sửa là script CREATE/DROP mở ra tab, không tự chạy.
- **Sinh dữ liệu test**: dòng dữ liệu thực tế cho một bảng (tên tiếng Việt/Anh, email, số điện thoại, địa chỉ, số
  tiền, ngày); khoá ngoại lấy giá trị có thật ở bảng cha. Script INSERT mở ra tab mới để xem trước rồi mới chạy.
  Email dùng tên miền ví dụ dành riêng; số điện thoại theo định dạng thật nên có thể trùng số của người khác — đừng
  dùng chúng để gửi tin nhắn.

</details>

<details>
<summary><b>Chạy query và kết quả</b></summary>

- F5 / Ctrl+E chạy vùng chọn hoặc cả tab, Shift+F5 chạy câu lệnh tại con trỏ; tách batch `GO` / `GO n`; huỷ query;
  nhiều result set.
- **Chạy trên nhiều database** (Ctrl+Shift+F5): mỗi dòng có cột `Database`, gộp lại nếu cùng cấu trúc.
- **Lưới kết quả ảo hoá**: sắp xếp, lọc, copy kèm header, **copy dạng INSERT / CSV / IN list / Markdown / JSON /
  HTML**, lưu CSV. Shift+click chọn vùng trong một cột để xem Count / Sum / Avg / Min / Max.
- **Excel**: mở kết quả bằng Excel hoặc lưu .xlsx (số vẫn là số, có header và filter).
- **Biểu đồ** cột, đường, tròn; copy thành ảnh.
- **So sánh kết quả**: ghim một kết quả, chạy lại rồi so sánh — dòng thêm, bớt, thay đổi theo cột khoá bạn chọn.
- **Kế hoạch thực thi ước tính** (Ctrl+L): dạng cây, % chi phí từng bước, cảnh báo scan cả bảng, key lookup, chuyển
  kiểu ngầm, thiếu điều kiện join, gợi ý index còn thiếu kèm câu CREATE INDEX; không chạy query.
- **Sửa dữ liệu trên lưới** (✎ Edit với kết quả từ một bảng có khoá chính): sửa ô, thêm/xoá dòng, xem trước câu
  lệnh; Save chạy trong một transaction, nếu dòng đã bị người khác sửa trong lúc đó thì không lưu gì.
- **Activity**: các phiên, câu đang chạy, đang chờ gì, transaction mở, chuỗi blocking (ai chặn ai); huỷ query hoặc
  kill phiên sau khi xác nhận.
- Màu tab theo kết nối. Mật khẩu lưu trong Windows Credential Manager.

</details>

<details>
<summary><b>Phím tắt</b></summary>

| Phím | Chức năng |
|---|---|
| F5 / Ctrl+E | Execute |
| Shift+F5 | Execute câu lệnh hiện tại |
| Ctrl+Shift+F5 | Chạy trên nhiều database |
| Alt+Break | Huỷ query |
| Ctrl+L | Kế hoạch thực thi ước tính |
| Ctrl+K, Ctrl+Y | Format |
| Tab sau `*` | Mở rộng thành danh sách cột |
| F2 | Đổi tên biến / alias / CTE |
| F12 / Alt+F12 | Mở định nghĩa object / Script as ALTER |
| Ctrl+B, Ctrl+W | Expand wildcards |
| Ctrl+B, Ctrl+Q | Qualify object names |
| F8 | Object Explorer |
| Ctrl+Alt+D | Object Search |
| Ctrl+Shift+H | Query History |
| F1 | Bảng lệnh (mọi thao tác) |
| Ctrl+N / Ctrl+W / Ctrl+Tab | Tab mới / đóng / chuyển |
| Ctrl+Shift+T | Mở lại tab vừa đóng |
| Ctrl+R | Ẩn/hiện kết quả |

</details>

## Ảnh chụp màn hình

<table>
  <tr>
    <td width="50%"><img src="../images/schema-compare.png" alt="So sánh schema: các object khác nhau giữa hai database, xem khác biệt song song"></td>
    <td width="50%"><img src="../images/plan.png" alt="Kế hoạch thực thi ước tính với chi phí từng bước, cạnh Object Explorer"></td>
  </tr>
  <tr>
    <td align="center"><b>So sánh schema</b> — xem khác biệt rồi nhận script đồng bộ sắp theo phụ thuộc</td>
    <td align="center"><b>Kế hoạch thực thi</b> — chi phí từng bước và cảnh báo, không cần chạy query</td>
  </tr>
  <tr>
    <td width="50%"><img src="../images/diagram.png" alt="Sơ đồ ER của database được sắp xếp tự động"></td>
    <td width="50%"><img src="../images/completion.png" alt="Gợi ý JOIN theo khoá ngoại"></td>
  </tr>
  <tr>
    <td align="center"><b>Sơ đồ ER</b> — cả database hoặc một bảng và các bảng liên quan</td>
    <td align="center"><b>Gợi ý code</b> — JOIN kèm điều kiện <code>ON</code> từ khoá ngoại</td>
  </tr>
</table>

## Sqlotus hoạt động thế nào

- **Bộ máy SQL** (`crates/sqlotus-core`) là thư viện Rust thuần, không IO: tokenizer chịu lỗi (hiểu cả query đang gõ
  dở), formatter, gợi ý code, phân tích code, refactor, kế hoạch thực thi và so sánh schema.
- **Driver** (`crates/sqlotus-db`) kết nối thẳng tới từng server — tiberius cho SQL Server, tokio-postgres và
  mysql_async — và nạp metadata cho gợi ý và phân tích.
- **Ứng dụng** dùng [Tauri 2](https://tauri.app): backend Rust gọn nhẹ với giao diện React + Monaco. Bộ cài dưới 10 MB.
- **Mọi thứ nằm trên máy bạn**: kết nối, lịch sử và cài đặt lưu trong file SQLite trên máy; mật khẩu nằm trong Windows
  Credential Manager.

## Câu hỏi thường gặp

<details>
<summary><b>Sqlotus có miễn phí không?</b></summary>

Có. Sqlotus là mã nguồn mở theo giấy phép MIT, không cần tài khoản, không thu phí, không có bản trả phí.
</details>

<details>
<summary><b>Dữ liệu hay query của tôi có bị gửi đi đâu không?</b></summary>

Không. Sqlotus không có telemetry và chỉ kết nối tới các database server bạn chọn. Xem
[chính sách bảo mật](../../.github/SECURITY.md) để biết dữ liệu được lưu thế nào.
</details>

<details>
<summary><b>Sqlotus có tự thay đổi dữ liệu mà không hỏi không?</b></summary>

Không. Script được sinh ra (đồng bộ schema, sửa index, dữ liệu test, sửa dòng) đều mở ra để bạn xem trước và không bao
giờ tự chạy. Câu lệnh thiếu `WHERE`, `TRUNCATE` và `DROP` luôn hỏi trước; script đổi nhiều dòng hỏi trước khi commit;
kết nối production xác nhận mọi thay đổi.
</details>

<details>
<summary><b>Hỗ trợ những phiên bản database nào?</b></summary>

Bộ test chạy với SQL Server 2022, PostgreSQL 17 và MySQL 8.4. Các phiên bản cũ hơn thường vẫn dùng được; nếu gặp lỗi,
hãy [tạo issue](https://github.com/tuancuongpham/Sqlotus/issues).
</details>

<details>
<summary><b>Sqlotus có thay thế công cụ quản trị database không?</b></summary>

Không. Sqlotus tập trung vào viết, chạy và hiểu SQL. Các việc quản trị server — backup, job, người dùng và phân quyền —
vẫn làm trong các công cụ chuyên cho việc đó.
</details>

<details>
<summary><b>Có chạy trên macOS hoặc Linux không?</b></summary>

Chưa. Windows authentication và Connection Doctor dùng API của Windows; phần còn lại có thể chuyển sang nền tảng khác
sau này.
</details>

<details>
<summary><b>Tôi có import được snippet của riêng mình không?</b></summary>

Có. **Options → Snippets → Import snippets…** đọc một thư mục các file `.json`, mỗi file một snippet, gồm `prefix`,
`description`, `body` (dùng placeholder `$name$` và `$CURSOR$`) và `placeholders`.
</details>

## Giới hạn hiện tại

- SQL Server: chưa hiển thị thông báo `PRINT`; số dòng thay đổi chỉ có với script chỉ thay đổi dữ liệu (driver tiberius
  chưa cung cấp cho batch khác).
- So sánh schema coi đổi tên cột/bảng là xoá + thêm; identity / cột computed khác nhau phải sửa tay.
- Hiện chỉ chạy trên Windows.
- Chưa hỗ trợ kết nối named pipe (LocalDB); dùng TCP.

## Phát triển

Yêu cầu: Windows 10/11, Rust (MSVC toolchain), Node 22+ và Visual Studio Build Tools (C++).

```bash
npm install
npx tauri dev
```

[CONTRIBUTING.md](../../.github/CONTRIBUTING.md) mô tả cấu trúc dự án, các bước kiểm tra trước khi gửi pull request
và cách test với database thật chạy bằng Docker. Các thay đổi được ghi trong [CHANGELOG.md](../../CHANGELOG.md).

## Miễn trừ trách nhiệm

Sqlotus được cung cấp nguyên trạng, không kèm bảo hành. Hãy xem lại script được sinh ra trước khi chạy, nhất là trên
database production.

SQL Server là thương hiệu của Microsoft, MySQL của Oracle và PostgreSQL của PostgreSQL Community Association. Các tên
này chỉ được nhắc để cho biết Sqlotus kết nối tới database nào; Sqlotus là dự án độc lập, không liên kết và không được
các bên trên bảo trợ.

## Đóng góp

Hoan nghênh báo lỗi, ý tưởng và pull request — xem [CONTRIBUTING.md](../../.github/CONTRIBUTING.md) và
[Code of Conduct](../../.github/CODE_OF_CONDUCT.md). Lỗi bảo mật xin báo riêng theo hướng dẫn trong
[SECURITY.md](../../.github/SECURITY.md).

## Giấy phép

[MIT](../../LICENSE). Sqlotus dùng các crate và gói npm mã nguồn mở; giấy phép của chúng được tập hợp trong
[licenses/THIRD_PARTY_LICENSES.md](../../licenses/THIRD_PARTY_LICENSES.md) và được cài kèm ứng dụng.
