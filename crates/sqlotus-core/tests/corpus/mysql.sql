-- Cases of valid MySQL 8, separated by "-- @case" lines. Object names need not
-- exist: tests transform each case and check its syntax only.
-- @case select joins backticks
select c.id, c.`name`, count(*) as orders, sum(o.total) total from `shop`.`customers` c inner join orders o on o.customer_id = c.id left join regions r on r.code = c.region and r.active = 1 where c.name like 'Nguyễn%' and (o.total > 100 or o.status in ('new', 'paid')) group by c.id, c.name having count(*) > 2 order by total desc limit 5, 10
-- @case cte window
with recursive tree (id, parent_id, lvl) as (select id, parent_id, 0 from nodes where parent_id is null union all select n.id, n.parent_id, t.lvl + 1 from nodes n join tree t on t.id = n.parent_id) select id, row_number() over (partition by parent_id order by id) rn, sum(lvl) over w running from tree window w as (order by id rows between unbounded preceding and current row)
-- @case upsert replace ignore
insert into customers (id, name) values (1, 'Trần Thị B'), (2, 'O\'Brien') as new on duplicate key update name = new.name;
insert ignore into tags (name) values ('x');
replace into settings (k, v) values ('a', 'b');
-- @case multi table update delete
update orders o join customers c on c.id = o.customer_id set o.status = 'late' where c.region = 'VN';
delete o from orders o left join customers c on c.id = o.customer_id where c.id is null;
-- @case procedure with delimiter
DELIMITER //
create procedure save_order(in p_customer int, in p_total decimal(12, 2), out p_id int)
begin
    declare done int default 0;
    declare v_id int;
    declare cur cursor for select id from orders where customer_id = p_customer;
    declare continue handler for not found set done = 1;
    declare exit handler for sqlexception begin rollback; resignal; end;
    start transaction;
    insert into orders (customer_id, total) values (p_customer, p_total);
    set p_id = last_insert_id();
    open cur;
    read_loop: loop
        fetch cur into v_id;
        if done then leave read_loop; end if;
    end loop;
    close cur;
    if p_total > 1000 then select 'big'; elseif p_total = 0 then select 'zero'; else select 'ok'; end if;
    commit;
end //
DELIMITER ;
-- @case function and trigger with delimiter
DELIMITER $$
create function order_total(p_customer int) returns decimal(12, 2) deterministic reads sql data
begin
    declare v decimal(12, 2);
    select coalesce(sum(total), 0) into v from orders where customer_id = p_customer;
    return v;
end $$
create trigger t_orders before insert on orders for each row
begin
    if new.total < 0 then signal sqlstate '45000' set message_text = 'negative $$ total'; end if;
    set new.created_at = now();
end$$
DELIMITER ;
-- @case table ddl
create table `order lines` (`line_no` int unsigned not null, order_id int not null, qty int not null default 1, price decimal(12, 2) not null, amount decimal(14, 2) generated always as (qty * price) stored, status enum('new', 'paid', 'void') not null default 'new', flags set('a', 'b') default null, note text, `user` varchar(50) character set utf8mb4 collate utf8mb4_bin, updated timestamp not null default current_timestamp on update current_timestamp, primary key (order_id, `line_no`), key ix_status (status), fulltext key ft_note (note), constraint fk_lines_orders foreign key (order_id) references orders (id) on delete cascade, constraint ck_qty check (qty > 0)) engine = InnoDB default charset = utf8mb4 comment = 'lines';
alter table `order lines` add column extra json null after note, modify column note mediumtext, drop index ix_status, add index ix_user (`user`(10));
-- @case strings and comments
select 'C:\\temp' as path, "double quoted" as dq, 'it\'s' as q1, 'it''s' as q2, _utf8mb4'x' as intro, X'0A0B' as hex, b'1010' as bits, '' as blank, '-- not comment' as c1, '# nor this' as c2, '/* nor */' as c3, 'a;b' as semi; # hash comment
select 5--2 as arithmetic, 5 -- real comment
, 6 /* block */ , /*!80000 7 + */ 8;
-- @case variables
set @a := 1, @b = 'x';
select @a := @a + 1 as next, @b;
select id into @last from orders order by id desc limit 1;
-- @case json and operators
select data->'$.name' as raw_name, data->>'$.name' as name, json_extract(data, '$.a[0]') as a0, 7 div 2, 7 mod 2, true xor false, null <=> null, name regexp '^N', name rlike 'x' from docs
-- @case group concat interval
select customer_id, group_concat(distinct status order by status separator ', ') as statuses, date_add(now(), interval 1 day) as tomorrow, now() - interval 2 hour as earlier from orders group by customer_id with rollup
-- @case show describe explain
show tables;
show columns from orders;
describe orders;
explain select * from orders where id = 1;
-- @case locks hints
lock tables orders read, customers write;
unlock tables;
select straight_join o.id from orders o force index (primary) join customers c use index (ix_name) on c.id = o.customer_id;
-- @case prepared statements
prepare s from 'select * from orders where id = ?';
set @id = 1;
execute s using @id;
deallocate prepare s;
-- @case transactions
start transaction;
savepoint s1;
update orders set total = total where false;
rollback to savepoint s1;
release savepoint s1;
commit;
-- @case exists in union
select id from customers c where exists (select 1 from orders o where o.customer_id = c.id) and c.id not in (select customer_id from blocked where customer_id is not null) union all select id from archived order by id
-- @case numbers
select 1e10, .5, 1.5e-3, -3, 0x1F, 12345678901234567890
