# Sqlotus

[English](README.md) · **Tiếng Việt**

Ứng dụng desktop soạn SQL nhanh, lấy cảm hứng từ Redgate SQL Prompt / SQL Search / SQL History.
Hỗ trợ SQL Server, PostgreSQL, MySQL/MariaDB.

Rust (Tauri 2) + React/TypeScript + Monaco Editor.

## Tính năng

- **Autocomplete theo ngữ cảnh**: keyword, schema/bảng/view/cột/proc/function, hiểu alias, CTE,
  derived table, `#temp`, `@table`; tự gán alias; gợi ý `JOIN … ON` từ foreign key; gợi ý điều kiện sau `ON`;
  `EXEC` chèn sẵn tham số; `INSERT INTO t (` gợi ý cột; biến `@x`.
- **Sinh câu lệnh như SQL Prompt**: chọn bảng sau `INSERT INTO` → sinh danh sách cột + `VALUES` có chú thích kiểu
  (Tab nhảy giữa các giá trị); `EXEC proc` → tham số mỗi dòng kèm kiểu; `GROUP BY` → gợi ý các cột không phải aggregate.
- **Mở rộng `*`**: gõ `SELECT *` (hoặc `t.*`) rồi nhấn **Tab** để thay bằng danh sách cột.
- **Hover** xem cột/kiểu của bảng, cột, biến; **gợi ý tham số** khi gõ hàm/proc.
- **Phân tích code** (gạch chân khi gõ): UPDATE/DELETE thiếu WHERE, `SELECT *`, `= NULL`, kiểu deprecated, biến không dùng,
  object/cột không tồn tại, chuỗi tiếng Việt thiếu tiền tố `N`, `NOT IN (SELECT …)` trên cột cho phép NULL,
  hàm bọc cột trong WHERE/ON (`YEAR(NgayTao) = 2024` không dùng được index), so sánh gây chuyển kiểu ngầm
  (`cot_varchar = N'x'`, `cot_chuoi = 123`). **Cảnh báo trước khi chạy** UPDATE/DELETE không WHERE, TRUNCATE, DROP.
- **Kết nối read-only**: chặn INSERT/UPDATE/DELETE/MERGE/DDL/GRANT, vẫn chạy được SELECT (tab có 🔒).
  **Nhắc transaction đang mở**: tab hiện **TRAN**, thanh công cụ có nút Commit / Roll back, đóng tab sẽ hỏi.
- **Hỏi trước khi lưu thay đổi lớn**: script chỉ INSERT/UPDATE/DELETE được chạy trong transaction; nếu đổi quá
  số dòng giới hạn (mặc định 1.000, Options → Safety) hoặc lỗi giữa chừng thì hỏi Commit hay Roll back; ít hơn thì
  tự commit và báo số dòng. **Manual commit** theo từng kết nối: thay đổi nằm trong transaction tới khi bấm Commit.
- **Biến chưa khai báo**: chạy `… WHERE Id = @Id` mà chưa `DECLARE` sẽ hỏi giá trị (và kiểu), tự khai báo, nhớ giá trị.
- **Kế hoạch thực thi ước tính** (Ctrl+L): dạng cây, % chi phí từng bước, cảnh báo scan cả bảng, key lookup,
  chuyển kiểu ngầm, thiếu điều kiện join, gợi ý index còn thiếu kèm câu CREATE INDEX (SQL Server, PostgreSQL, MySQL;
  không chạy query).
- **Lưới kết quả**: mở bằng Excel / lưu .xlsx, vẽ biểu đồ (cột, đường, tròn; copy ảnh), ghim một kết quả rồi
  **so sánh** với kết quả sau (dòng thêm, bớt, thay đổi theo cột khoá).
- **Connection Doctor**: khi kết nối lỗi tự giải thích nguyên nhân (từ chối kết nối, timeout, sai host/instance,
  sai đăng nhập, chứng chỉ, database, Windows Auth); với SQL Server trên máy này kiểm tra service, TCP/IP, port,
  SQL Browser, chế độ xác thực. Sửa một chạm: bật TCP/IP, khởi động service, bật SQL Browser, bật Mixed Mode
  (Windows hỏi quyền Administrator), dùng đúng port/host, chuyển Windows Auth, tin chứng chỉ, mở port trên Windows
  Firewall, bật lại login bị disable; sửa xong tự kết nối lại. Lỗi **18456** trên máy này: đọc lý do thật từ Windows
  event log (sai mật khẩu, login không tồn tại, bị disable, hết hạn…). Nút **Check server** kiểm tra khi chưa có lỗi.
- **Thông báo cho mọi thao tác và cài đặt**: đang kết nối/đã kết nối, chạy xong (thời gian, số dòng),
  lỗi kèm nút Diagnose/Retry, format, copy, lưu CSV, lưu kết nối, lưu cài đặt, khôi phục tab.
- **Thao tác** (chuột phải hoặc F1): Expand wildcards, Qualify object names, Insert semicolons, Remove square brackets,
  Uppercase/Lowercase keywords; **F12** mở định nghĩa object, **Alt+F12** script ALTER.
- **Object Explorer** (F8, mở sẵn khi khởi động): lọc object/cột, Select top 100, script SELECT/INSERT/UPDATE/DELETE/EXEC/CREATE/ALTER,
  **Find invalid objects** (view/proc/function/trigger tham chiếu tới bảng/cột không còn tồn tại).
- **Đổi tên** (F2) biến, alias, CTE ở mọi chỗ dùng.
- **Sơ đồ ER**: bảng và khoá ngoại của cả database hoặc một bảng cùng các bảng liên quan, tự sắp xếp; kéo thả, zoom,
  tô sáng quan hệ, copy ảnh hoặc lưu SVG (Object Explorer → Diagram).
- **Used by**: view, proc, function, trigger và khoá ngoại nào đang dùng bảng/cột/module, kèm dòng — xem trước khi
  đổi tên hoặc xoá.
- **So sánh schema** (thanh công cụ → Compare): hai database trên cùng hoặc khác kết nối — bảng (cột, kiểu, NULL,
  default, identity, computed), khoá chính, index, unique, khoá ngoại, check, view, procedure, function, trigger; xem
  khác biệt từng object song song. Script đồng bộ sắp theo phụ thuộc (bỏ khoá ngoại trước, tạo lại sau cùng, tạo schema
  còn thiếu), ghi cảnh báo mất dữ liệu ở đầu, mở ra tab đã kết nối tới database đích, không tự chạy. Object chỉ có ở
  đích không được chọn sẵn (phải tự chọn nếu muốn xoá). SQL Server cần quyền VIEW DEFINITION; thiếu quyền thì từ chối so
  sánh vì object bị ẩn sẽ trông như không tồn tại.
- **Sức khoẻ index**: index server gợi ý còn thiếu, index trùng/chồng lấn, index không ai đọc, bảng bị đọc toàn bộ, heap
  lớn. Cách sửa là script CREATE/DROP mở ra tab, không tự chạy.
- **Sửa dữ liệu trên lưới** (✎ Edit với kết quả từ một bảng có khoá chính): sửa ô, thêm/xoá dòng, xem trước câu lệnh;
  Save chạy trong một transaction, nếu dòng đã bị người khác sửa trong lúc đó thì không lưu gì.
- **Activity**: các phiên, câu đang chạy, đang chờ gì, transaction mở, chuỗi blocking (ai chặn ai); huỷ query hoặc kill
  phiên sau khi xác nhận.
- **Môi trường kết nối** Dev/Test/Staging/Production: Production có tab đỏ, nhãn PROD, manual commit và hỏi lại (ghi rõ
  server) trước mọi thay đổi. Copy kết quả dạng Markdown, JSON, HTML.
- **Sinh dữ liệu test**: dòng dữ liệu thực tế cho một bảng (tên tiếng Việt/Anh, email, số điện thoại, địa chỉ, số tiền,
  ngày); khoá ngoại lấy giá trị có thật ở bảng cha, khoá không identity nối tiếp giá trị lớn nhất. Script INSERT mở ra
  tab mới để xem trước rồi mới chạy (Object Explorer → chọn bảng → Test data…).
- **Gợi ý xuyên database**: `OtherDb.` → schema, `OtherDb.dbo.` → bảng, cột của bảng ở database khác.
- **Chạy trên nhiều database** (Ctrl+Shift+F5): kết quả có cột `Database`, gộp lại nếu cùng cấu trúc.
- **Snippet**: lần chạy đầu tự import snippet của SQL Prompt (`%LOCALAPPDATA%\Red Gate\SQL Prompt *\Snippets`).
- **Format SQL** (Ctrl+K, Ctrl+Y) với tuỳ chọn: chữ hoa/thường keyword, căn lề kiểu Redgate hoặc xếp chồng,
  dấu phẩy đầu/cuối, thụt JOIN, ON/AND/OR xuống dòng. Formatter tự kiểm tra không làm đổi token nào.
- **SQL Search** (Ctrl+Alt+D): tìm theo tên object, tên cột, nội dung view/proc/function/trigger,
  một hoặc tất cả database; xem định nghĩa có highlight, mở ra tab mới.
- **SQL History** (Ctrl+Shift+H): tự lưu phiên bản các tab (định kỳ và mỗi lần Execute), tìm full-text,
  khôi phục tab đã đóng (Ctrl+Shift+T), so sánh (diff) với phiên bản trước hoặc với tab hiện tại.
  Các tab đang mở được khôi phục khi mở lại app.
- Chạy query: F5 / Ctrl+E (vùng chọn hoặc cả tab), Shift+F5 (câu lệnh tại con trỏ), tách batch `GO` / `GO n`,
  huỷ query, lưới kết quả ảo hoá (sắp xếp theo cột, lọc, copy Ctrl+C / Ctrl+Shift+C kèm header;
  Copy as INSERT / CSV / IN list; lưu file CSV; Shift+click chọn vùng trong một cột để xem Count/Sum/Avg/Min/Max), tab Messages.
- Màu tab theo kết nối (ví dụ đỏ cho production). Mật khẩu lưu trong Windows Credential Manager.

## Phím tắt

| Phím | Chức năng |
|---|---|
| F5 / Ctrl+E | Execute |
| Shift+F5 | Execute câu lệnh hiện tại |
| Ctrl+L | Kế hoạch thực thi ước tính |
| Alt+Break | Huỷ query |
| Ctrl+K, Ctrl+Y | Format |
| Tab sau `*` | Mở rộng thành danh sách cột |
| F12 / Alt+F12 | Mở định nghĩa object / Script as ALTER |
| Ctrl+B, Ctrl+W | Expand wildcards |
| Ctrl+B, Ctrl+Q | Qualify object names |
| F8 | Object Explorer |
| F2 | Đổi tên biến / alias / CTE |
| Ctrl+Shift+F5 | Chạy trên nhiều database |
| F1 | Bảng lệnh (mọi thao tác) |
| Ctrl+Alt+D | SQL Search |
| Ctrl+Shift+H | SQL History |
| Ctrl+N / Ctrl+W / Ctrl+Tab | Tab mới / đóng / chuyển |
| Ctrl+Shift+T | Mở lại tab vừa đóng |
| Ctrl+R | Ẩn/hiện kết quả |

## Phát triển

Yêu cầu: Rust (MSVC), Node 22+, Visual Studio Build Tools (C++).

```bash
npm install
npx tauri dev
```

```bash
cargo test --workspace
```

```bash
npx tauri build
```

Cấu trúc:

- `crates/sqlotus-core` — lexer chịu lỗi, formatter, completion, phân tích scope, snippet, search (không IO).
- `crates/sqlotus-db` — kết nối & chạy query (tiberius / tokio-postgres / mysql_async), nạp metadata.
- `crates/sqlotus-store` — SQLite: kết nối, cài đặt, snippet, history (FTS5 trigram), cache catalog.
- `src-tauri` — Tauri commands, phiên kết nối theo tab.
- `src` — giao diện React + Monaco.
- `scripts/` — test end-to-end điều khiển app qua DevTools Protocol của WebView2 (`ui-smoke.mjs`, `e2e-*.mjs`) và
  database test chạy bằng Docker (`test-db/`).

Test với database thật: đặt biến môi trường `SQLOTUS_TEST_MSSQL` / `SQLOTUS_TEST_PG` / `SQLOTUS_TEST_MYSQL`
(xem `crates/sqlotus-db/tests/live.rs`).

## Giới hạn hiện tại

- SQL Server: chưa hiển thị thông báo `PRINT`; số dòng thay đổi chỉ có với script chỉ thay đổi dữ liệu (driver tiberius
  chưa cung cấp cho batch khác).
- So sánh schema coi đổi tên cột/bảng là xoá + thêm; identity / cột computed khác nhau phải sửa tay.
- Hiện chỉ chạy trên Windows.
- Chưa hỗ trợ kết nối named pipe (LocalDB); dùng TCP.
- Tính năng AI Assistant chưa làm.

## Giấy phép

[MIT](LICENSE)
