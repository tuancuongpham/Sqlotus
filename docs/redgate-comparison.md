# So sánh với Redgate SQL Prompt / SQL Search / SQL History

Trạng thái: ✅ có · 🟡 có một phần · ❌ chưa có. Cột "Mới" đánh dấu tính năng thêm trong đợt này.

## Gõ nhanh (SQL Prompt – code completion)

| Tính năng Redgate | SqlSupportQuery | Mới | Ghi chú |
|---|---|---|---|
| Gợi ý keyword, schema, bảng, view, cột, proc, function | ✅ | | |
| Hiểu alias, CTE, derived table, `#temp`, `@table` | ✅ | | |
| Tự gán alias cho bảng | ✅ | | `OrderDetails` → `od` |
| Gợi ý `JOIN … ON` theo khoá ngoại, gợi ý điều kiện sau `ON` | ✅ | | |
| `SELECT *` + Tab → danh sách cột | ✅ | | |
| **`INSERT INTO bảng` → sinh danh sách cột + `VALUES` có chú thích kiểu** | ✅ | ✅ | Bỏ cột identity/computed/rowversion; Tab nhảy giữa các giá trị |
| `EXEC proc` → tham số mỗi dòng, giá trị mẫu, chú thích kiểu, `(has default)` | ✅ | ✅ | Đọc giá trị mặc định T-SQL từ định nghĩa proc |
| `GROUP BY` → gợi ý các cột không phải aggregate | ✅ | ✅ | |
| Cụm từ khoá (`INNER JOIN`, `IS NOT NULL`, `BEGIN TRY`…) | ✅ | ✅ | |
| Gợi ý tham số khi gõ hàm/proc (parameter info) | ✅ | ✅ | Hàm built-in + proc/function trong DB |
| Tooltip định nghĩa object (hover) | ✅ | ✅ | Bảng: danh sách cột; cột: kiểu; biến: kiểu |
| Snippets, placeholder, `$SELECTEDTEXT$`, import snippet SQL Prompt | ✅ | | |
| Column picker (chọn cột bằng checkbox) | ❌ | | Ý tưởng #2 |
| Gợi ý object ở database khác (`OtherDb.dbo.`) | ❌ | | Ý tưởng #4 |

## Format & thao tác trên code

| Tính năng Redgate | SqlSupportQuery | Mới | Ghi chú |
|---|---|---|---|
| Format SQL theo style | 🟡 | | Có các tuỳ chọn chính; Redgate có hàng trăm tuỳ chỉnh |
| Apply casing (chữ hoa/thường keyword) | ✅ | ✅ | Không đổi bố cục |
| Expand wildcards | ✅ | ✅ | Ctrl+B, Ctrl+W |
| Qualify object names | ✅ | ✅ | Ctrl+B, Ctrl+Q |
| Insert semicolons | ✅ | ✅ | |
| Remove square brackets | ✅ | ✅ | Add brackets: chưa |
| Comment/uncomment | ✅ | | Ctrl+/ |
| Smart rename (object, cột, biến) | ❌ | | Ý tưởng #1 |
| Encapsulate as stored procedure, Split table, Summarize script | ❌ | | |

## Phân tích code & an toàn

| Tính năng Redgate | SqlSupportQuery | Mới | Ghi chú |
|---|---|---|---|
| Gạch chân object/cột không tồn tại | ✅ | ✅ | SQ006, SQ007 |
| Code analysis rules | 🟡 | ✅ | 8 luật: UPDATE/DELETE thiếu WHERE, `SELECT *`, `= NULL`, kiểu deprecated, biến không dùng, object/cột sai, chuỗi Unicode thiếu `N` (luật riêng cho tiếng Việt). Redgate có ~100 luật |
| Execution warnings (UPDATE/DELETE không WHERE) | ✅ | ✅ | Thêm TRUNCATE, DROP TABLE/DATABASE |
| Find invalid objects (toàn database) | ❌ | | Ý tưởng #5 |

## Điều hướng & object

| Tính năng Redgate/SSMS | SqlSupportQuery | Mới | Ghi chú |
|---|---|---|---|
| Go to definition / Script as ALTER | ✅ | ✅ | F12 / Alt+F12 |
| Object Explorer | 🟡 | ✅ | F8: lọc, xem cột, Select top 100, script SELECT/INSERT/UPDATE/DELETE/EXEC/CREATE/ALTER |
| Tab color theo kết nối | ✅ | | |

## Lưới kết quả

| Tính năng | SqlSupportQuery | Mới | Ghi chú |
|---|---|---|---|
| Script as INSERT từ kết quả | ✅ | ✅ | `INSERT INTO #Results …`, chia lô 1000 dòng |
| Copy as IN (…) list, CSV | ✅ | ✅ | |
| Copy with headers | ✅ | | |
| Open in Excel, lọc/sắp xếp trên lưới | ❌ | | Ý tưởng #7 |

## SQL Search

| Tính năng | SqlSupportQuery | Ghi chú |
|---|---|---|
| Tìm theo tên, cột, nội dung định nghĩa | ✅ | |
| Nhiều database, exact match, match case, lọc loại object | ✅ | |
| Xem định nghĩa có highlight, mở ra tab | ✅ | |
| Tìm trên nhiều server cùng lúc, wildcard/boolean | ❌ | |

## SQL History

| Tính năng | SqlSupportQuery | Ghi chú |
|---|---|---|
| Tự lưu phiên bản (định kỳ + khi Execute), tìm full-text | ✅ | |
| Khôi phục tab đã đóng, tab mở lại khi khởi động | ✅ | |
| So sánh (diff) giữa các phiên bản | ❌ | Ý tưởng #3 |

## Ý tưởng tính năng tiếp theo (xếp theo giá trị / công sức)

1. **Rename biến/alias trong script** (F2) — đổi tất cả chỗ dùng trong batch; về sau mở rộng sang object DB (sinh script `sp_rename` + cập nhật module phụ thuộc).
2. **Column picker** trong gợi ý: sau `SELECT` chọn nhiều cột bằng checkbox thay vì gõ từng cột.
3. **Diff phiên bản History** (Monaco diff editor) — xem đã sửa gì so với lần chạy trước.
4. **Gợi ý cross-database** (`OtherDb.dbo.Table`) bằng cách nạp catalog của DB khác theo yêu cầu.
5. **Find invalid objects toàn DB**: dùng `sys.sql_expression_dependencies` để tìm proc/view tham chiếu tới object/cột không còn tồn tại.
6. **Execution plan**: `SET SHOWPLAN_XML` / `EXPLAIN (FORMAT JSON)` và hiển thị dạng cây, cảnh báo scan/thiếu index.
7. **Lưới kết quả nâng cao**: lọc, sắp xếp, xuất Excel, biểu đồ nhanh, so sánh hai result set.
8. **Chạy trên nhiều database** (multi-tenant): chọn danh sách DB, chạy cùng script, gộp kết quả có cột `DatabaseName`.
9. **Sinh dữ liệu test** cho một bảng (theo kiểu dữ liệu, tôn trọng khoá ngoại).
10. **Hỏi giá trị biến khi chạy**: phát hiện `@param` chưa khai báo và hiện form nhập trước khi Execute.
11. **Bảng điều khiển vận hành**: session đang chạy, blocking/lock, query chậm (`sys.dm_exec_requests`, `pg_stat_activity`).
12. **AI Assistant** (đã tạm hoãn): giải thích/viết/sửa SQL dựa trên schema đã nạp — hạ tầng hover/catalog hiện có dùng lại được.
