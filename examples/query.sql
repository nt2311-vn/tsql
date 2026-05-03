create table users(id integer primary key, name text not null);
insert into users(name) values ('ada'), ('grace');
select id, name from users order by id;
