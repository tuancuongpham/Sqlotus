# So sánh với Redgate SQL Prompt / SQL Search / SQL History

Trạng thái: ✅ có · 🟡 có một phần · ❌ chưa có. Cột "Mới" đánh dấu tính năng thêm trong đợt này.

## Gõ nhanh (SQL Prompt – code completion)

| Tính năng Redgate | Sqlotus | Mới | Ghi chú |
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
| Gợi ý object ở database khác (`OtherDb.dbo.`) | ✅ | ✅ | Tự nạp catalog của database được nhắc tới |

## Format & thao tác trên code

| Tính năng Redgate | Sqlotus | Mới | Ghi chú |
|---|---|---|---|
| Format SQL theo style | 🟡 | | Có các tuỳ chọn chính; Redgate có hàng trăm tuỳ chỉnh |
| Apply casing (chữ hoa/thường keyword) | ✅ | ✅ | Không đổi bố cục |
| Expand wildcards | ✅ | ✅ | Ctrl+B, Ctrl+W |
| Qualify object names | ✅ | ✅ | Ctrl+B, Ctrl+Q |
| Insert semicolons | ✅ | ✅ | |
| Remove square brackets | ✅ | ✅ | Add brackets: chưa |
| Comment/uncomment | ✅ | | Ctrl+/ |
| Smart rename biến / alias / CTE trong script | ✅ | ✅ | F2 |
| Smart rename object trong database (cập nhật module phụ thuộc) | ❌ | | |
| Encapsulate as stored procedure, Split table, Summarize script | ❌ | | |

## Phân tích code & an toàn

| Tính năng Redgate | Sqlotus | Mới | Ghi chú |
|---|---|---|---|
| Gạch chân object/cột không tồn tại | ✅ | ✅ | SQ006, SQ007 |
| Code analysis rules | 🟡 | ✅ | 11 luật: UPDATE/DELETE thiếu WHERE, `SELECT *`, `= NULL`, kiểu deprecated, biến không dùng, object/cột sai, chuỗi Unicode thiếu `N` (luật riêng cho tiếng Việt), `NOT IN` trên cột NULL, hàm bọc cột trong WHERE, chuyển kiểu ngầm. Redgate có ~100 luật |
| Execution warnings (UPDATE/DELETE không WHERE) | ✅ | ✅ | Thêm TRUNCATE, DROP TABLE/DATABASE |
| Find invalid objects (toàn database) | ✅ | ✅ | Object Explorer → "Find invalid objects" |

## Điều hướng & object

| Tính năng Redgate/SSMS | Sqlotus | Mới | Ghi chú |
|---|---|---|---|
| Go to definition / Script as ALTER | ✅ | ✅ | F12 / Alt+F12 |
| Object Explorer | 🟡 | ✅ | F8: lọc, xem cột, Select top 100, script SELECT/INSERT/UPDATE/DELETE/EXEC/CREATE/ALTER |
| Tab color theo kết nối | ✅ | | |

## Lưới kết quả

| Tính năng | Sqlotus | Mới | Ghi chú |
|---|---|---|---|
| Script as INSERT từ kết quả | ✅ | ✅ | `INSERT INTO #Results …`, chia lô 1000 dòng |
| Copy as IN (…) list, CSV | ✅ | ✅ | |
| Copy with headers | ✅ | | |
| Lọc, sắp xếp trên lưới, lưu CSV (Excel mở được) | ✅ | ✅ | |
| Mở trực tiếp bằng Excel, biểu đồ | ❌ | | |

## SQL Search

| Tính năng | Sqlotus | Ghi chú |
|---|---|---|
| Tìm theo tên, cột, nội dung định nghĩa | ✅ | |
| Nhiều database, exact match, match case, lọc loại object | ✅ | |
| Xem định nghĩa có highlight, mở ra tab | ✅ | |
| Tìm trên nhiều server cùng lúc, wildcard/boolean | ❌ | |

## SQL History

| Tính năng | Sqlotus | Ghi chú |
|---|---|---|
| Tự lưu phiên bản (định kỳ + khi Execute), tìm full-text | ✅ | |
| Khôi phục tab đã đóng, tab mở lại khi khởi động | ✅ | |
| So sánh (diff) giữa các phiên bản / với tab hiện tại | ✅ | Monaco diff editor |

## Ngoài Redgate

| Tính năng | Ghi chú |
|---|---|
| Chạy một script trên nhiều database (Ctrl+Shift+F5) | Kết quả có cột `Database`, gộp lại nếu cùng cấu trúc |
| Cảnh báo chuỗi tiếng Việt thiếu tiền tố `N` | Tránh dữ liệu bị lưu thành `?` |
| Hỗ trợ PostgreSQL và MySQL | Redgate SQL Prompt chỉ hỗ trợ SQL Server |

## An toàn & trải nghiệm (đợt Sqlotus)

| Tính năng | Có ở | Sqlotus | Ghi chú |
|---|---|---|---|
| Connection Doctor: chẩn đoán + sửa lỗi kết nối (bật TCP/IP, service, SQL Browser, Mixed Mode, port) | — | ✅ | Không công cụ nào trong danh sách có |
| Thông báo cho mọi thao tác/cài đặt | — | ✅ | Lỗi có nút Diagnose/Retry |
| Kết nối read-only | DataGrip | ✅ | Chặn ở phía client trước khi gửi |
| Nhắc transaction đang mở | dbForge SQL Complete | ✅ | Nhãn TRAN, Commit/Roll back, hỏi khi đóng tab |
| Tổng/trung bình vùng chọn trên lưới | dbForge (trả phí), SSMSBoost | ✅ | Shift+click trong một cột |
| Object Explorer mở sẵn | SSMS | ✅ | |
| Hỏi trước khi commit thay đổi lớn, manual commit | DataGrip, dbForge | ✅ | v0.2.0 |
| Hỏi giá trị biến chưa khai báo | SSMS (template parameters) | ✅ | v0.2.0 |
| Execution plan ước tính + gợi ý index | SSMS, Plan Explorer | ✅ | v0.2.0, dạng cây |
| Mở Excel, biểu đồ, so sánh result set | dbForge (trả phí) | ✅ | v0.2.0 |
| Giải thích lỗi 18456, mở firewall | — | ✅ | v0.2.0 |

Danh sách pain point và ứng viên tính năng: `docs/pain-points.md`.

## Ý tưởng tiếp theo

1. **Rename object trong database**: sinh script `sp_rename` và cập nhật các view/proc tham chiếu.
2. **Column picker** trong gợi ý: chọn nhiều cột bằng checkbox.
3. **Execution plan**: `SET SHOWPLAN_XML` / `EXPLAIN (FORMAT JSON)` hiển thị dạng cây, cảnh báo scan/thiếu index.
4. **Biểu đồ nhanh và mở Excel** từ lưới kết quả; so sánh hai result set.
5. **Sinh dữ liệu test** cho một bảng (theo kiểu dữ liệu, tôn trọng khoá ngoại).
6. **Hỏi giá trị biến khi chạy** cho `@param` chưa khai báo.
7. **Bảng điều khiển vận hành**: session đang chạy, blocking/lock, query chậm.
8. **AI Assistant** (đang tạm hoãn): giải thích/viết/sửa SQL dựa trên schema đã nạp.
