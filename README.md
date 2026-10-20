# SqlSupportQuery

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
  object/cột không tồn tại, chuỗi tiếng Việt thiếu tiền tố `N`. **Cảnh báo trước khi chạy** UPDATE/DELETE không WHERE, TRUNCATE, DROP.
- **Thao tác** (chuột phải hoặc F1): Expand wildcards, Qualify object names, Insert semicolons, Remove square brackets,
  Uppercase/Lowercase keywords; **F12** mở định nghĩa object, **Alt+F12** script ALTER.
- **Object Explorer** (F8): lọc object/cột, Select top 100, script SELECT/INSERT/UPDATE/DELETE/EXEC/CREATE/ALTER.
- **Snippet**: lần chạy đầu tự import snippet của SQL Prompt (`%LOCALAPPDATA%\Red Gate\SQL Prompt *\Snippets`).
- **Format SQL** (Ctrl+K, Ctrl+Y) với tuỳ chọn: chữ hoa/thường keyword, căn lề kiểu Redgate hoặc xếp chồng,
  dấu phẩy đầu/cuối, thụt JOIN, ON/AND/OR xuống dòng. Formatter tự kiểm tra không làm đổi token nào.
- **SQL Search** (Ctrl+Alt+D): tìm theo tên object, tên cột, nội dung view/proc/function/trigger,
  một hoặc tất cả database; xem định nghĩa có highlight, mở ra tab mới.
- **SQL History** (Ctrl+Shift+H): tự lưu phiên bản các tab (định kỳ và mỗi lần Execute), tìm full-text,
  khôi phục tab đã đóng (Ctrl+Shift+T). Các tab đang mở được khôi phục khi mở lại app.
- Chạy query: F5 / Ctrl+E (vùng chọn hoặc cả tab), Shift+F5 (câu lệnh tại con trỏ), tách batch `GO` / `GO n`,
  huỷ query, lưới kết quả ảo hoá (copy Ctrl+C / Ctrl+Shift+C kèm header; Copy as INSERT / CSV / IN list), tab Messages.
- Màu tab theo kết nối (ví dụ đỏ cho production). Mật khẩu lưu trong Windows Credential Manager.

## Phím tắt

| Phím | Chức năng |
|---|---|
| F5 / Ctrl+E | Execute |
| Shift+F5 | Execute câu lệnh hiện tại |
| Alt+Break | Huỷ query |
| Ctrl+K, Ctrl+Y | Format |
| Tab sau `*` | Mở rộng thành danh sách cột |
| F12 / Alt+F12 | Mở định nghĩa object / Script as ALTER |
| Ctrl+B, Ctrl+W | Expand wildcards |
| Ctrl+B, Ctrl+Q | Qualify object names |
| F8 | Object Explorer |
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

- `crates/sqlq-core` — lexer chịu lỗi, formatter, completion, phân tích scope, snippet, search (không IO).
- `crates/sqlq-db` — kết nối & chạy query (tiberius / tokio-postgres / mysql_async), nạp metadata.
- `crates/sqlq-store` — SQLite: kết nối, cài đặt, snippet, history (FTS5 trigram), cache catalog.
- `src-tauri` — Tauri commands, phiên kết nối theo tab.
- `src` — giao diện React + Monaco.
- `scripts/ui-smoke.mjs`, `scripts/e2e-db.mjs`, `scripts/e2e-features.mjs` — test UI qua DevTools Protocol của WebView2.
- `docs/redgate-comparison.md` — so sánh với Redgate và các ý tưởng tiếp theo.

Test với database thật: đặt biến môi trường `SQLQ_TEST_MSSQL` / `SQLQ_TEST_PG` / `SQLQ_TEST_MYSQL`
(xem `crates/sqlq-db/tests/live.rs`).

## Giới hạn hiện tại

- SQL Server: chưa hiển thị thông báo `PRINT` và số dòng bị ảnh hưởng của lệnh DML (thư viện tiberius chưa cung cấp).
- Chưa hỗ trợ kết nối named pipe (LocalDB); dùng TCP.
- Tính năng AI Assistant chưa làm.
