# Đề xuất tính năng mới — đánh giá triển vọng (chờ xác nhận)

Tổng hợp tháng 10/2026 từ khảo sát người dùng, diễn đàn và tài liệu sản phẩm (nguồn ở cuối). Các mục dưới đây
**chưa được làm**; bạn chọn mục nào thì mới implement.

Cách chấm: **Triển vọng** = mức đau của người dùng × số người gặp × chưa có công cụ miễn phí tốt.
**Công sức**: Thấp (≤ 1 ngày), TB (2–4 ngày), Cao (≥ 1 tuần). **Rủi ro**: khả năng làm sai dữ liệu hoặc khó đúng
trên cả 3 loại database.

## Bối cảnh mới (2025–2026)

- **Azure Data Studio đã ngừng hỗ trợ từ 28/02/2026.** Microsoft chuyển người dùng sang VS Code, nhưng VS Code còn
  thiếu SQL Agent, profiling, một số tác vụ quản trị, và schema compare vẫn "đang phát triển" — người dùng macOS/Linux
  bị đẩy về SSMS (chỉ chạy Windows).
- **Khảo sát DataGrip 2025:** người dùng chưa hài lòng phàn nàn nhiều nhất về trình soạn code (33%), database explorer
  (32%), giao diện (24%) và việc đọc metadata chậm; **92% chia sẻ câu SQL, 64% chia sẻ kết quả** trong nhóm; 57% đã
  dùng AI để viết/sửa query.
- **VS Code MSSQL (11/2025)** vừa thêm "Edit Data": sửa dữ liệu trên lưới, kiểm tra kiểu/khoá/NULL trước khi lưu, xem
  trước script INSERT/UPDATE/DELETE — dấu hiệu đây là nhu cầu lớn.
- **Yêu cầu nhiều phiếu nhất cho SSMS** vẫn là debugger, dark mode, khôi phục một bảng từ backup, chạy câu lệnh tại con
  trỏ (Sqlotus đã có dark mode và Shift+F5).
- **Xử lý blocking/deadlock** vẫn dựa vào script cộng đồng (sp_WhoIsActive, sp_BlitzLock) phải cài vào server, và
  nhiều người chỉ dùng chúng ở mức cơ bản.

## Bảng đề xuất

| # | Tính năng | Pain point giải quyết | Đã có ở (thường trả phí) | Triển vọng | Công sức | Rủi ro |
|---|---|---|---|---|---|---|
| 1 | **Sửa dữ liệu trên lưới** (Edit data) | Sửa vài dòng phải viết UPDATE tay; dễ sai WHERE | dbForge, DataGrip, DBeaver, VS Code (preview) | ⭐⭐⭐ Cao | TB | TB |
| 2 | **Theo dõi phiên & blocking** (Activity monitor) | Query treo không biết ai chặn ai; SSMS Activity Monitor nặng | sp_WhoIsActive (phải cài), dbForge Monitor, Redgate Monitor | ⭐⭐⭐ Cao | TB | Thấp |
| 3 | **Xem phụ thuộc / phân tích ảnh hưởng** | Đổi tên/xoá cột không biết proc/view nào hỏng | Redgate SQL Dependency Tracker | ⭐⭐⭐ Cao | Thấp–TB | Thấp |
| 4 | **Sức khoẻ index**: thiếu / không dùng / trùng lặp | Query chậm, ghi chậm vì thừa index | SSMS reports rời rạc, sp_BlitzIndex (phải cài) | ⭐⭐⭐ Cao | Thấp–TB | Thấp |
| 5 | **So sánh schema** 2 database + script đồng bộ | Dev/test/prod lệch cấu trúc; Redgate SQL Compare đắt; VS Code chưa có | Redgate SQL Compare, dbForge Schema Compare | ⭐⭐⭐ Cao | Cao | Cao |
| 6 | **Chia sẻ query + kết quả**: copy Markdown/JSON/HTML, link file `.sqlotus` | 92% chia sẻ SQL, 64% chia sẻ kết quả (JetBrains) | DataGrip (một phần) | ⭐⭐ TB–Cao | Thấp | Thấp |
| 7 | **Top query chậm / hồi quy plan** (Query Store, pg_stat_statements, performance_schema) | Không biết query nào tốn tài nguyên nhất, plan đổi xấu | SSMS Query Store reports, dbForge Profiler | ⭐⭐ TB–Cao | TB | Thấp |
| 8 | **Xem deadlock gần đây** (system_health) | Deadlock xảy ra rồi mới biết, khó đọc XML | sp_BlitzLock (phải cài), SentryOne | ⭐⭐ TB | TB | Thấp |
| 9 | **SQL Agent jobs** (xem, lịch sử, chạy) | Người rời ADS bị đẩy về SSMS chỉ để xem job | SSMS | ⭐⭐ TB | TB | TB |
| 10 | **So sánh dữ liệu** 2 bảng/2 database theo khoá + script đồng bộ | Kiểm tra dữ liệu sau migrate/ETL | Redgate SQL Data Compare, dbForge Data Compare | ⭐⭐ TB | TB | Cao |
| 11 | **Smart rename object** (sp_rename + sửa view/proc phụ thuộc) | Đổi tên làm hỏng module | Redgate SQL Prompt | ⭐⭐ TB | TB–Cao | Cao |
| 12 | **Môi trường & chính sách kết nối** (nhóm Dev/Test/Prod, mặc định màu + read-only + manual commit cho Prod) | Nhầm cửa sổ production | DataGrip (màu, read-only) | ⭐⭐ TB | Thấp | Thấp |
| 13 | **Bản macOS/Linux** | ADS ngừng; SSMS chỉ Windows | DBeaver, DataGrip, DbGate | ⭐⭐ TB | TB | TB |
| 14 | **AI assistant** (đang tạm hoãn theo yêu cầu) | 57% đã dùng AI cho SQL | SSMS Copilot, DataGrip AI, Redgate | ⭐⭐⭐ Cao | Cao | TB |
| 15 | Debugger T-SQL | Yêu cầu nhiều phiếu nhất cho SSMS | dbForge Studio | ⭐ Thấp (khả thi) | Rất cao | Cao |
| 16 | Khôi phục một bảng từ backup | Yêu cầu nhiều phiếu cho SQL Server | Công cụ chuyên dụng | ⭐ Thấp (khả thi) | Rất cao | Cao |

## Đề xuất thứ tự (nếu bạn đồng ý)

1. **#3 Phụ thuộc** và **#4 Sức khoẻ index** — rẻ, an toàn (chỉ đọc), dùng lại catalog/định nghĩa đã có, giá trị cao.
2. **#2 Phiên & blocking** — chỉ đọc + nút Kill có xác nhận; bù chỗ trống sau khi ADS ngừng.
3. **#1 Sửa dữ liệu trên lưới** — làm theo mô hình an toàn: thay đổi chờ, kiểm tra kiểu/NULL/khoá, xem script trước,
   lưu trong transaction (dùng lại cơ chế hỏi commit của v0.2.0). Chỉ cho bảng có khoá chính.
4. **#6 Chia sẻ** và **#12 Môi trường** — nhỏ, làm kèm.
5. **#5 So sánh schema** — làm theo 2 giai đoạn: (a) chỉ xem khác biệt (cột, index, ràng buộc, module) → (b) sinh
   script đồng bộ có xem trước.

Mục #15, #16 không khuyến nghị: công sức rất lớn so với giá trị cho một ứng dụng độc lập.

## Chi tiết từng đề xuất

### 1. Sửa dữ liệu trên lưới
- Double-click ô để sửa; thêm/xoá dòng; ô đã sửa tô màu; Undo từng ô.
- Kiểm tra trước khi lưu: kiểu dữ liệu, độ dài, NOT NULL, khoá chính trùng.
- Nút **Xem script** hiển thị đúng các câu UPDATE/INSERT/DELETE sẽ chạy (WHERE theo khoá chính).
- Lưu = chạy script trong transaction; số dòng ảnh hưởng phải khớp số dòng đã sửa, nếu không thì rollback và báo.
- Chỉ bật khi kết quả lấy từ đúng một bảng có khoá chính; kết nối read-only thì khoá.

### 2. Theo dõi phiên & blocking
- Danh sách phiên đang chạy: người dùng, máy, database, thời gian, câu lệnh, chờ gì; cây **ai chặn ai**.
- SQL Server: `sys.dm_exec_requests/sessions/sql_text`; PostgreSQL: `pg_stat_activity` + `pg_blocking_pids`;
  MySQL: `performance_schema` / `information_schema.processlist`.
- Kill phiên (có xác nhận, ghi rõ phiên nào); tự làm mới mỗi vài giây; mở plan của câu đang chạy.
- Cần quyền VIEW SERVER STATE — Connection Doctor giải thích nếu thiếu.

### 3. Phụ thuộc / phân tích ảnh hưởng
- Chuột phải bảng/cột/proc → **Ai đang dùng?**: view, proc, function, trigger tham chiếu (từ định nghĩa đã nạp và
  `sys.sql_expression_dependencies`), kèm dòng tham chiếu.
- Trước khi DROP/ALTER: cảnh báo "3 procedure dùng cột này".

### 4. Sức khoẻ index
- Index thiếu (DMV missing index, gom theo bảng, kèm CREATE INDEX), index không được đọc (usage stats) nhưng tốn ghi,
  index trùng/chứa nhau, bảng heap lớn. PostgreSQL: `pg_stat_user_indexes`; MySQL: `sys.schema_unused_indexes`.
- Chỉ đọc; mọi script đều mở ra tab để xem trước.

### 5. So sánh schema
- Chọn nguồn/đích (2 database cùng hoặc khác server). Hiện cây khác biệt: thêm/bớt/sửa cho bảng (cột, kiểu, NULL,
  default), index, khoá, view/proc/function (diff định nghĩa bằng Monaco diff có sẵn).
- Giai đoạn 2: script đồng bộ theo thứ tự phụ thuộc, cảnh báo thao tác mất dữ liệu (xoá cột, thu hẹp kiểu).

### 6. Chia sẻ
- Copy kết quả dạng Markdown (dán vào Jira/GitHub/Teams), JSON, HTML.
- Lưu/mở file `.sqlotus` gồm query + kết nối (không có mật khẩu) + kết quả gần nhất.

## Nguồn

- https://blog.jetbrains.com/datagrip/2025/08/04/datagrip-and-database-tools-ux-survey-2-results/
- https://www.getgalaxy.io/blog/datagrips-ux-survey-shows-developers-want-faster-smarter-more-collaborative-sql-tools
- https://www.devclass.com/databases/2025/02/11/microsoft-drops-azure-data-studio-in-favour-of-visual-studio-code-extension-despite-missing-features/1629157
- https://www.directionsonmicrosoft.com/reports/azure-data-studio-is-retiring-what-you-need-to-do-and-when/
- https://devblogs.microsoft.com/azure-sql/vscode-mssql-edit-data/
- https://dbeaver.com/docs/dbeaver/25.3/Data-Editor/
- https://www.brentozar.com/archive/2022/05/the-top-feature-requests-for-sql-server/
- https://www.brentozar.com/archive/2025/04/whats-new-in-sql-server-2025-2/
- https://learn.microsoft.com/en-us/ssms/roadmap
- https://www.brentozar.com/archive/2018/11/how-to-troubleshoot-blocking-and-deadlocking-with-scripts-and-tools/
- https://straightpathsql.com/5-common-sql-server-problems-to-troubleshoot-with-sp_whoisactive/
- https://blog.jetbrains.com/datagrip/2021/08/19/transition-from-ssms-to-datagrip-10-tips/
- https://redlib.hackliberty.org/r/SQL/comments/1ku1ees/dbeaver_vs_ssms_and_why
- https://www.g2.com/products/dbforge-studio-for-sql-server-2018-12-04/reviews/dbforge-studio-for-sql-server-review-13086165
- https://www.devart.com/dbforge/Sql/studio/sql-server-database-compare.html
- https://alternativeto.net/software/red-gate-sql-compare
- https://documentation.red-gate.com/sp10/release-notes-and-other-versions/sql-prompt-10-13-release-notes
- https://www.red-gate.com/hub/product-learning/sql-data-generator/generate-fake-test-data-sql-server
- https://forum.red-gate.com/discussion/18090/foreign-key-generator-request
- https://support.microsoft.com/en-US/Visio/reverse-engineer-an-existing-database-into-a-database-model
- https://www.sqlservercentral.com/?p=148581
- https://www.dbi-services.com/blog/sql-server-2022-parameter-sensitive-plan-optimization/
