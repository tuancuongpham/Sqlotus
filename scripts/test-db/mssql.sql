CREATE DATABASE Shop;
GO
USE Shop;
GO
CREATE SCHEMA sales;
GO
CREATE TABLE dbo.Customers (Id INT IDENTITY PRIMARY KEY, Name NVARCHAR(100) NOT NULL, Country CHAR(2) NULL, CreatedAt DATETIME2 NOT NULL DEFAULT SYSUTCDATETIME());
CREATE TABLE dbo.Orders (Id INT IDENTITY PRIMARY KEY, CustomerId INT NOT NULL REFERENCES dbo.Customers(Id), Total DECIMAL(12,2) NOT NULL, OrderDate DATE NOT NULL);
CREATE TABLE dbo.OrderItems (Id INT IDENTITY PRIMARY KEY, OrderId INT NOT NULL CONSTRAINT FK_OrderItems_Orders REFERENCES dbo.Orders(Id), Sku VARCHAR(20) NOT NULL, Qty INT NOT NULL, Price MONEY NOT NULL);
CREATE TABLE sales.Regions (Code CHAR(2) PRIMARY KEY, Name NVARCHAR(50) NOT NULL);
GO
INSERT INTO dbo.Customers (Name, Country) VALUES (N'Nguyễn Văn A', 'VN'), (N'Trần Thị B', 'VN'), (N'John Smith', 'US');
INSERT INTO dbo.Orders (CustomerId, Total, OrderDate) VALUES (1, 150.50, '2024-05-01'), (1, 20, '2024-06-02'), (3, 999.99, '2024-07-03');
INSERT INTO dbo.OrderItems (OrderId, Sku, Qty, Price) VALUES (1, 'A-1', 2, 50.25), (1, 'B-2', 1, 50), (3, 'C-3', 1, 999.99);
INSERT INTO sales.Regions VALUES ('VN', N'Việt Nam'), ('US', N'United States');
GO
CREATE VIEW dbo.vCustomerTotals AS SELECT c.Id, c.Name, SUM(o.Total) AS Total FROM dbo.Customers c JOIN dbo.Orders o ON o.CustomerId = c.Id GROUP BY c.Id, c.Name;
GO
CREATE PROCEDURE dbo.GetCustomerOrders @CustomerId INT, @From DATE = NULL AS
BEGIN
  SET NOCOUNT ON;
  SELECT o.Id, o.Total, o.OrderDate FROM dbo.Orders o WHERE o.CustomerId = @CustomerId AND (@From IS NULL OR o.OrderDate >= @From);
END
GO
CREATE FUNCTION dbo.fnOrderCount (@CustomerId INT) RETURNS INT AS BEGIN RETURN (SELECT COUNT(*) FROM dbo.Orders WHERE CustomerId = @CustomerId) END
GO
