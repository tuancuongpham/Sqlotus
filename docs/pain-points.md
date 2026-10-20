# Pain point khi làm việc với SQL và hướng xử lý trong Sqlotus

Tổng hợp từ diễn đàn, blog và tài liệu sản phẩm (nguồn ở cuối trang). Mỗi mục ghi: vấn đề người dùng gặp,
công cụ trả phí nào đã giải quyết, Sqlotus đang ở đâu.

Trạng thái: ✅ đã có · 🆕 thêm trong v0.2.0 · 💡 ứng viên để cân nhắc.

## 1. Kết nối thất bại mà không biết vì sao

**Vấn đề.** Lỗi "A network-related or instance-specific error…" có nhiều nguyên nhân: service SQL Server
chưa chạy, TCP/IP bị tắt, named instance cần SQL Server Browser, firewall chặn 1433/UDP 1434, sai port,
server chỉ cho Windows Authentication. Lỗi đăng nhập 18456 chỉ ghi lý do thật (state 5/8/38…) trong error
log của server, người dùng chỉ thấy "Login failed".

**Công cụ trả phí.** Hầu như không có; SSMS, DataGrip, dbForge chỉ hiển thị nguyên văn lỗi.

**Sqlotus.**
- 🆕 **Connection Doctor**: khi kết nối/Test thất bại tự mở hộp chẩn đoán. Phân loại lỗi (từ chối kết nối,
  timeout, không tìm thấy host/instance, sai đăng nhập, chứng chỉ, database, Windows Auth) và với SQL Server
  trên máy này thì đọc trạng thái thật: service có chạy, TCP/IP bật chưa, port đang nghe, SQL Browser,
  chế độ xác thực.
- 🆕 Nút sửa một chạm: **bật TCP/IP**, **khởi động service**, **bật SQL Browser**, **bật Mixed Mode**
  (Windows hỏi quyền Administrator), **dùng đúng port**, **đổi host**, **chuyển sang Windows Auth**,
  **tin chứng chỉ**, **chọn database khác**. Sửa xong tự thử kết nối lại và báo kết quả.
- 🆕 Lỗi 18456 trên máy này: đọc lý do thật từ Windows event log (sai mật khẩu, login không tồn tại, bị disable,
  hết hạn, database không mở được); login bị disable có nút bật lại (qua Windows Auth, cần sysadmin). Server ở máy
  khác: hướng dẫn đọc `xp_readerrorlog` và ý nghĩa từng state.
- 🆕 Mở port trên Windows Firewall (TCP + UDP 1434 cho named instance, chỉ mạng domain/private).
- 🆕 Nút **Check server** chạy kiểm tra khi chưa có lỗi (chuẩn bị cho máy khác kết nối).

## 2. Không biết thao tác đã xong hay chưa

**Vấn đề.** Nhiều thao tác im lặng: lưu kết nối, format, copy, lưu cài đặt… Người dùng phải đoán.

**Sqlotus.** 🆕 Thông báo (toast) cho mọi thao tác và cài đặt: đang kết nối → đã kết nối, chạy xong
(thời gian, số dòng), lỗi kèm nút **Diagnose**/**Retry**, format/refactor ("Ctrl+Z để hoàn tác"), copy,
lưu CSV, lưu/xoá kết nối, lưu cài đặt, snippet, tìm kiếm, khôi phục tab, chuyển database. Lỗi giữ lại tới
khi đóng; thông báo thành công tự ẩn.

## 3. Mất script khi SSMS treo hoặc đóng nhầm tab

**Vấn đề.** SSMS hay treo/crash, auto-recovery không đáng tin, tab `SQLQuery1.sql` chưa lưu mất luôn.

**Công cụ trả phí.** Redgate SQL History, ApexSQL Complete, dbForge (tab history), SSMS Plus.

**Sqlotus.** ✅ Mọi tab tự lưu phiên bản (định kỳ + khi Execute), mở lại tab đã đóng (Ctrl+Shift+T),
khôi phục tab khi khởi động lại, tìm full-text, diff. 🆕 Báo "Restored N tab(s)" khi mở app.

## 4. Lỡ tay sửa dữ liệu production

**Vấn đề.** UPDATE/DELETE quên WHERE, chạy nhầm cửa sổ production, quên COMMIT làm khoá bảng của
người khác.

**Công cụ trả phí.** Redgate SQL Prompt (execution warnings, tab color), dbForge SQL Complete (transaction
reminder), DataGrip (read-only connection, manual commit), MySQL `--safe-updates`.

**Sqlotus.**
- ✅ Cảnh báo trước khi chạy UPDATE/DELETE không WHERE, TRUNCATE, DROP; màu tab theo kết nối.
- 🆕 **Kết nối read-only**: chặn INSERT/UPDATE/DELETE/MERGE/DDL/GRANT…, vẫn cho SELECT; tab có biểu tượng 🔒.
- 🆕 **Nhắc transaction đang mở**: sau khi chạy nếu còn transaction (`@@TRANCOUNT`, `pg_stat_activity`,
  `innodb_trx`) thì tab hiện nhãn **TRAN** và thông báo có nút **Commit**/**Roll back**; đóng tab sẽ hỏi
  Commit / Roll back / Huỷ.
- 🆕 Chế độ **manual commit** theo kết nối; nút Commit / Roll back trên thanh công cụ.
- 🆕 **Hỏi khi thay đổi vượt ngưỡng** (mặc định 1.000 dòng): chạy trong transaction, đếm số dòng, hỏi Commit/Roll back;
  ít hơn thì tự commit. MySQL báo số dòng khớp điều kiện (giống SQL Server/PostgreSQL).

## 5. Xem nhanh tổng/trung bình trong lưới kết quả

**Vấn đề.** Muốn biết tổng một cột phải viết lại query hoặc copy sang Excel.

**Công cụ trả phí.** dbForge SQL Complete (bản trả phí), SSMSBoost, DataGrip.

**Sqlotus.** 🆕 Chọn một ô, Shift+click ô khác cùng cột → thanh trạng thái hiện Count/Sum/Avg/Min/Max
(cột số) hoặc Count/Distinct/Null; Ctrl+C copy cả vùng.
- 🆕 Mở bằng Excel / lưu .xlsx, vẽ biểu đồ nhanh, ghim và so sánh hai result set.

## 6. Gõ SQL chậm, nhớ tên cột

**Công cụ trả phí.** Redgate SQL Prompt, dbForge SQL Complete, ApexSQL Complete, DataGrip.

**Sqlotus.** ✅ Gợi ý theo ngữ cảnh, JOIN theo khoá ngoại, sinh INSERT/EXEC, mở rộng `SELECT *`,
cross-database, snippet (import từ SQL Prompt). Xem `docs/redgate-comparison.md`.

## 7. Lỗi hiệu năng và logic khó thấy

**Vấn đề.** Implicit conversion, hàm bọc cột có index, `NOT IN` với NULL, `SELECT *`, thiếu index.

**Công cụ trả phí.** Redgate SQL Prompt code analysis (~100 luật), SQL Monitor, Plan Explorer.

**Sqlotus.** ✅ 8 luật (gồm chuỗi Unicode thiếu `N`).
- 🆕 Luật SQ009 (`NOT IN` trên cột cho phép NULL), SQ010 (hàm bọc cột trong WHERE/ON), SQ011 (chuyển kiểu ngầm).
- 🆕 Execution plan ước tính dạng cây (Ctrl+L), cảnh báo scan/key lookup/chuyển kiểu, gợi ý CREATE INDEX.

## 8. Tìm object, hiểu database lạ

**Công cụ trả phí.** Redgate SQL Search (miễn phí), SSMS Plus, DataGrip.

**Sqlotus.** ✅ SQL Search, Object Explorer, Find invalid objects. 🆕 Explorer mở sẵn khi khởi động.
- 💡 Sơ đồ quan hệ bảng (ER) từ khoá ngoại; tìm trên nhiều server cùng lúc.

## Danh sách ứng viên

| # | Tính năng | Trạng thái |
|---|---|---|
| 1 | Giải thích lỗi 18456 bằng event log / error log | 🆕 v0.2.0 |
| 2 | Mở port Windows Firewall từ Connection Doctor | 🆕 v0.2.0 |
| 3 | Manual commit cho kết nối production | 🆕 v0.2.0 |
| 4 | Xác nhận khi ảnh hưởng quá N dòng | 🆕 v0.2.0 |
| 5 | Execution plan dạng cây + cảnh báo | 🆕 v0.2.0 |
| 6 | Thêm luật phân tích (NOT IN/NULL, hàm trên cột, implicit conversion) | 🆕 v0.2.0 |
| 7 | Biểu đồ nhanh / mở Excel / so sánh result set | 🆕 v0.2.0 |
| 8 | Hỏi giá trị cho `@param` chưa khai báo khi chạy | 🆕 v0.2.0 |
| 9 | Sơ đồ ER | 💡 |
| 10 | Sinh dữ liệu test cho bảng | 💡 |

## Nguồn

- https://learn.microsoft.com/en-us/answers/questions/1442100/how-to-fix-a-network-related-or-instance-specific
- https://technet.microsoft.com/en-us/library/ms190181
- https://www.bytebase.com/reference/sqlserver/error/18456-login-failed-for-user/
- https://www.netdata.cloud/guides/microsoft-sql-server/microsoft-sql-server-error-18456-login-failed/
- https://www.sqlskills.com/blogs/jonathan/recovering-scripts-after-ssms-crash/
- https://solutioncenter.apexsql.com/?p=9286
- https://www.devart.com/blog/recovering-closed-tabs-in-ssms.html
- https://www.devart.com/dbforge/sql/sqlcomplete
- https://www.componentsource.com/product/dbforge-sql-complete/about
- https://www.red-gate.com/hub/product-learning/sql-prompt/sql-prompt-safety-net-features-ssms-sql-history
- https://www.jetbrains.com/help/datagrip/configuring-database-connections.html
- https://www.bennadel.com/blog/3479-using-safe-updates-to-prevent-unbounded-update-and-delete-statements-in-mysql.htm
- https://www.brentozar.com/archive/2022/05/the-top-feature-requests-for-sql-server/
- https://github.com/wondisha/SSMSPlus/blob/master/README.md
- https://www.red-gate.com/simple-talk/databases/sql-server/t-sql-programming-sql-server/ten-common-sql-programming-mistakes
- https://databasehealth.com/sql-code-smells-implicit-conversions/
- https://www.capterra.com/p/236288/SQLTreeo-SSMS-AddIn/reviews/
