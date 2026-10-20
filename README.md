# SqlSupportQuery

Ứng dụng desktop soạn SQL nhanh, lấy cảm hứng từ Redgate SQL Prompt / SQL Search / SQL History.
Hỗ trợ SQL Server, PostgreSQL, MySQL/MariaDB.

Rust (Tauri 2) + React/TypeScript + Monaco Editor.

## Tính năng

- **Autocomplete theo ngữ cảnh**: keyword, schema/bảng/view/cột/proc/function, hiểu alias, CTE,
  derived table, `#temp`, `@table`; tự gán alias; gợi ý `JOIN … ON` từ foreign key; gợi ý điều kiện sau `ON`;
  `EXEC` chèn sẵn tham số; `INSERT INTO t (` gợi ý cột; biến `@x`.
- **Mở rộng `*`**: gõ `SELECT *` (hoặc `t.*`) rồi nhấn **Tab** để thay bằng danh sách cột.
- **Snippet**: lần chạy đầu tự import snippet của SQL Prompt (`%LOCALAPPDATA%\Red Gate\SQL Prompt *\Snippets`).
- **Format SQL** (Ctrl+K, Ctrl+Y) với tuỳ chọn: chữ hoa/thường keyword, căn lề kiểu Redgate hoặc xếp chồng,
  dấu phẩy đầu/cuối, thụt JOIN, ON/AND/OR xuống dòng. Formatter tự kiểm tra không làm đổi token nào.
- **SQL Search** (Ctrl+Alt+D): tìm theo tên object, tên cột, nội dung view/proc/function/trigger,
  một hoặc tất cả database; xem định nghĩa có highlight, mở ra tab mới.
- **SQL History** (Ctrl+Shift+H): tự lưu phiên bản các tab (định kỳ và mỗi lần Execute), tìm full-text,
  khôi phục tab đã đóng (Ctrl+Shift+T). Các tab đang mở được khôi phục khi mở lại app.
- Chạy query: F5 / Ctrl+E (vùng chọn hoặc cả tab), Shift+F5 (câu lệnh tại con trỏ), tách batch `GO` / `GO n`,
  huỷ query, lưới kết quả ảo hoá (copy Ctrl+C / Ctrl+Shift+C kèm header), tab Messages.
- Màu tab theo kết nối (ví dụ đỏ cho production). Mật khẩu lưu trong Windows Credential Manager.

## Phím tắt

| Phím | Chức năng |
|---|---|
| F5 / Ctrl+E | Execute |
| Shift+F5 | Execute câu lệnh hiện tại |
| Alt+Break | Huỷ query |
| Ctrl+K, Ctrl+Y | Format |
| Tab sau `*` | Mở rộng thành danh sách cột |
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
- `scripts/ui-smoke.mjs` — smoke test UI qua DevTools Protocol của WebView2.

Test với database thật: đặt biến môi trường `SQLQ_TEST_MSSQL` / `SQLQ_TEST_PG` / `SQLQ_TEST_MYSQL`
(xem `crates/sqlq-db/tests/live.rs`).

## Giới hạn hiện tại

- SQL Server: chưa hiển thị thông báo `PRINT` và số dòng bị ảnh hưởng của lệnh DML (thư viện tiberius chưa cung cấp).
- Chưa hỗ trợ kết nối named pipe (LocalDB); dùng TCP.
- Tính năng AI Assistant chưa làm.
