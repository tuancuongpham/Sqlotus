-- Cases of valid T-SQL, separated by "-- @case" lines. Object names need not
-- exist: tests transform each case and check its syntax only.
-- @case select joins
select c.Id, c.Name, count(*) as Orders, sum(o.Total) Total from dbo.Customers c inner join dbo.Orders o on o.CustomerId = c.Id left outer join dbo.Regions r on r.Code = c.Region and r.Active = 1 where c.Name like N'Nguyễn%' and (o.Total > 100 or o.Status in ('new', 'paid')) group by c.Id, c.Name having count(*) > 2 order by Total desc, c.Name
-- @case cte recursive
with Tree (Id, ParentId, Lvl) as (select Id, ParentId, 0 from dbo.Nodes where ParentId is null union all select n.Id, n.ParentId, t.Lvl + 1 from dbo.Nodes n join Tree t on t.Id = n.ParentId) select * from Tree option (maxrecursion 100)
-- @case window functions
select Id, row_number() over (partition by CustomerId order by OrderDate desc) as rn, sum(Total) over (order by OrderDate rows between unbounded preceding and current row) running, lag(Total, 1, 0) over (order by Id) prev from dbo.Orders
-- @case case expressions
select case when Total > 1000 then 'big' when Total > 100 then case Status when 'paid' then 'medium paid' else 'medium' end else 'small' end as Size, iif(Total > 0, 1, 0) Positive, choose(2, 'a', 'b') Pick from dbo.Orders
-- @case merge with output
merge dbo.Customers as t using (select Id, Name from dbo.Staging) as s on t.Id = s.Id when matched and t.Name <> s.Name then update set t.Name = s.Name when not matched by target then insert (Id, Name) values (s.Id, s.Name) when not matched by source then delete output $action, inserted.Id, deleted.Id;
-- @case pivot
select * from (select Region, Year, Total from dbo.Sales) s pivot (sum(Total) for Year in ([2023], [2024], [2025])) p
-- @case unpivot
select Id, Attr, Val from dbo.Wide unpivot (Val for Attr in (A, B, C)) u
-- @case apply
select c.Name, x.Total from dbo.Customers c cross apply (select top (1) o.Total from dbo.Orders o where o.CustomerId = c.Id order by o.Total desc) x outer apply dbo.fOrders(c.Id) f
-- @case insert forms
insert into dbo.Customers (Name, Code) values (N'Trần Thị B', 'B'), ('O''Brien', 'C');
insert dbo.Archive (Id, Name) select Id, Name from dbo.Customers where Id < 10;
insert into #t exec dbo.GetOrders @CustomerId = 1;
-- @case update delete joins
declare @gone table (Id int);
update o set o.Status = 'late' from dbo.Orders o join dbo.Customers c on c.Id = o.CustomerId where c.Region = 'VN';
delete o output deleted.Id into @gone (Id) from dbo.Orders o where not exists (select 1 from dbo.Customers c where c.Id = o.CustomerId)
-- @case procedure try catch
create procedure dbo.SaveOrder @CustomerId int, @Total decimal(12, 2) = 0, @Id int output
as
begin
    set nocount on;
    begin try
        begin tran;
        insert dbo.Orders (CustomerId, Total) values (@CustomerId, @Total);
        set @Id = scope_identity();
        commit;
    end try
    begin catch
        if @@trancount > 0 rollback;
        declare @msg nvarchar(4000) = error_message();
        raiserror (@msg, 16, 1);
        throw;
    end catch
end
GO
-- @case functions
create function dbo.fTotal (@CustomerId int) returns decimal(12, 2) as begin return (select sum(Total) from dbo.Orders where CustomerId = @CustomerId) end
GO
create function dbo.fOrders (@CustomerId int) returns table as return (select Id, Total from dbo.Orders where CustomerId = @CustomerId)
GO
create function dbo.fSplit (@s nvarchar(max)) returns @out table (Item nvarchar(100)) as begin insert @out select value from string_split(@s, ','); return; end
GO
-- @case trigger and view
create trigger dbo.tOrders on dbo.Orders after insert, update as begin set nocount on; update o set Changed = sysdatetime() from dbo.Orders o join inserted i on i.Id = o.Id; end
GO
create view dbo.vTotals with schemabinding as select CustomerId, count_big(*) as N, sum(Total) as Total from dbo.Orders group by CustomerId
GO
-- @case table ddl
create table dbo.[Order Lines] ([LineNo] int not null, OrderId int not null constraint FK_Lines_Orders references dbo.Orders (Id) on delete cascade, Qty int not null constraint DF_Qty default (1), Price money not null, Amount as (Qty * Price) persisted, [User] nvarchar(50) null, Note nvarchar(max) collate Latin1_General_CS_AS null, constraint PK_Lines primary key clustered (OrderId, [LineNo]), constraint CK_Qty check (Qty > 0));
create unique nonclustered index IX_Lines_User on dbo.[Order Lines] ([User]) include (Qty) where [User] is not null with (fillfactor = 90);
alter table dbo.[Order Lines] add Created datetime2(3) not null constraint DF_Created default sysdatetime();
alter table dbo.[Order Lines] alter column Note nvarchar(4000) null;
alter table dbo.[Order Lines] drop constraint CK_Qty;
-- @case temp tables and variables
declare @t table (Id int primary key, Name nvarchar(50));
create table #work (Id int, [Tên khách] nvarchar(50));
create table ##shared (Id int);
select * into #copy from dbo.Customers where 1 = 0;
insert @t (Id, Name) select Id, Name from #work;
drop table if exists #work, #copy;
-- @case dynamic sql with tricky strings
declare @sql nvarchar(max) = N'select ''-- not a comment'', ''/* nor this */'' from dbo.T where Name = @n
GO
select 1';
exec sp_executesql @sql, N'@n nvarchar(50)', @n = N'it''s';
select 'GO' as [GO], '' as Empty, N'Tiếng Việt' as Vn, '[not ident]' as Br, 'a;b' as Semi
-- @case brackets and quoted names
select [a]]b], [select], [Order], "Group", [Khách hàng].[Tên], [Order Lines].[LineNo], [x] from dbo.[Khách hàng] join dbo.[Order Lines] on 1 = 1 cross join [dbo].[x]
-- @case comments everywhere
/* header /* nested */ still header */
select /* inline */ Id -- trailing comment
    , Name /* between */ from dbo.Customers -- end
where Id = 1 /* last */
-- @case batches with go counts
select 1 as One
GO
print 'second batch'
GO 2
select 3
go
-- @case control flow
declare @i int = 0, @n int;
while @i < 10
begin
    set @i += 1;
    if @i % 2 = 0 continue;
    if @i > 7 break;
end
if exists (select 1 from dbo.Customers) select 'yes' else select 'no'
if @i = 1 begin print 'one' end else if @i = 2 begin print 'two' end else print 'many'
-- @case labels goto
declare @k int = 0;
again:
set @k = @k + 1;
if @k < 3 goto again;
select @k
-- @case cursor
declare c cursor local fast_forward for select Id from dbo.Customers;
declare @id int;
open c;
fetch next from c into @id;
while @@fetch_status = 0 begin fetch next from c into @id; end
close c;
deallocate c;
-- @case paging top hints
select top (10) with ties Id, Total from dbo.Orders with (nolock) order by Total desc;
select top 5 percent * from dbo.Orders;
select Id from dbo.Orders order by Id offset 20 rows fetch next 10 rows only option (recompile, maxdop 1);
-- @case xml json
select Id, Name from dbo.Customers for xml path('c'), root('cs');
select Id, Name from dbo.Customers for json path, include_null_values;
select j.* from openjson(N'[{"id":1,"name":"a"}]') with (id int '$.id', name nvarchar(50) '$.name') j;
select string_agg(Name, ', ') within group (order by Name) from dbo.Customers
-- @case numbers and literals
select 1e10 as E, .5 as Half, 0x0A0B as Bin, $12.50 as Money, -3 as Neg, 1.5E-3 as Small, N'' as EmptyN, cast('2024-01-31T10:00:00' as datetime2) as D, {d '2024-01-31'} as OdbcDate
-- @case semicolonless sequence
declare @a int
set @a = 1
select @a
update dbo.T set X = @a where Id = 1

delete from dbo.T where Id = 2
select @@rowcount
-- @case leading semicolon cte
;with x as (select 1 as n) select n from x
-- @case exec forms
exec dbo.GetOrders 1;
execute dbo.GetOrders @CustomerId = 2;
exec ('select 1');
declare @r int;
exec @r = dbo.SaveOrder @CustomerId = 1, @Total = 5.5, @Id = @r output;
-- @case sequences and create or alter
create sequence dbo.Seq as int start with 1 increment by 1;
GO
create or alter view dbo.vNext as select 1 as n
GO
select next value for dbo.Seq as n
GO
create or alter procedure dbo.p as select 1 as x
GO
-- @case truncate and transactions
begin transaction t1;
truncate table dbo.Staging;
save transaction sp1;
rollback transaction sp1;
commit transaction t1;
-- @case functions misc
select try_convert(int, '12'), try_cast('x' as int), format(sysdatetime(), 'yyyy-MM-dd'), datediff(day, '2024-01-01', getdate()), concat_ws('-', 'a', 'b'), coalesce(null, 1), nullif(1, 1), isnull(null, 'x'), len(N'ễ'), datalength(N'ễ'), left('abc', 2)
-- @case waitfor and set options
set xact_abort on;
set transaction isolation level read committed;
waitfor delay '00:00:00.010';
set identity_insert dbo.Customers on;
set identity_insert dbo.Customers off;
-- @case grouping sets
select Region, Year, sum(Total) from dbo.Sales group by grouping sets ((Region, Year), (Region), ()) ;
select Region, sum(Total) from dbo.Sales group by rollup (Region) having grouping(Region) = 0
-- @case scalar subqueries and exists
select Id, (select max(Total) from dbo.Orders o where o.CustomerId = c.Id) as MaxTotal from dbo.Customers c where exists (select * from dbo.Orders o where o.CustomerId = c.Id) and c.Id not in (select CustomerId from dbo.Blocked where CustomerId is not null)
-- @case union intersect except
select Id from dbo.A union all select Id from dbo.B except select Id from dbo.C intersect select Id from dbo.D order by Id
-- @case scope operators and methods
grant select on schema::dbo to public;
declare @g geography = geography::Point(47.65, -122.34, 4326);
select @g.Lat, @g.STAsText(), geometry::STGeomFromText('POINT(1 2)', 0).STX;
select 1 where 2 !< 1 and 1 !> 2 and 3 != 4 and 5 <> 6;
declare @x int = 1; set @x *= -2; set @x -= 1; set @x |= 4;
-- @case sqlcmd variables
:setvar Schema "dbo"
:setvar Suffix _2024
:on error exit
select o.Id, o.Total from $(Schema).Orders$(Suffix) o where o.Note = N'$(Schema)' -- $(Suffix)
go
  :setvar Suffix _2025
insert into [$(Schema)].[Orders$(Suffix)] (Id) select Id from dbo.Orders where 1 = 0;
