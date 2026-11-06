-- stockroom, migration 1: stores, the catalog, stock, orders, the stock ledger and the outbox.
--
-- Targets PostgreSQL 18: keys default to uuidv7() (PG 18) and stock.available is a virtual
-- generated column (PG 18). Constraint names are part of the contract with the Rust code:
-- src/error.rs turns them into messages for API clients, so renaming one means updating it.

CREATE TYPE order_status AS ENUM ('pending', 'paid', 'ready', 'collected', 'cancelled');

CREATE TYPE address AS (
    street   text,
    city     text,
    postcode text,
    country  char(2)
);

-- A stock-keeping unit: 3–12 upper-case letters or digits, optionally a dash and a variant.
CREATE DOMAIN sku AS text
    CONSTRAINT sku_format CHECK (VALUE ~ '^[A-Z0-9]{3,12}(-[A-Z0-9]{1,6})?$');

CREATE DOMAIN positive_quantity AS integer
    CONSTRAINT positive_quantity_check CHECK (VALUE > 0);

CREATE TABLE stores (
    id         uuid        PRIMARY KEY DEFAULT uuidv7(),
    code       text        NOT NULL
                           CONSTRAINT stores_code_key UNIQUE
                           CONSTRAINT stores_code_format CHECK (code ~ '^[a-z0-9][a-z0-9-]{1,31}$'),
    name       text        NOT NULL
                           CONSTRAINT stores_name_length CHECK (length(name) BETWEEN 1 AND 200),
    address    address     NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE products (
    id          uuid        PRIMARY KEY DEFAULT uuidv7(),
    sku         sku         NOT NULL CONSTRAINT products_sku_key UNIQUE,
    name        text        NOT NULL
                            CONSTRAINT products_name_length CHECK (length(name) BETWEEN 1 AND 200),
    price_cents bigint      NOT NULL CONSTRAINT products_price_positive CHECK (price_cents > 0),
    tags        text[]      NOT NULL DEFAULT '{}',
    attributes  jsonb       NOT NULL DEFAULT '{}',
    version     integer     NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX products_tags_idx ON products USING gin (tags);

-- on_hand and reserved are plain integers rather than a domain, because a virtual generated
-- column may not use user-defined types (PostgreSQL 18 refuses the table otherwise).
CREATE TABLE stock (
    store_id   uuid    NOT NULL REFERENCES stores ON DELETE CASCADE,
    product_id uuid    NOT NULL REFERENCES products ON DELETE CASCADE,
    on_hand    integer NOT NULL DEFAULT 0
                       CONSTRAINT stock_on_hand_not_negative CHECK (on_hand >= 0),
    reserved   integer NOT NULL DEFAULT 0
                       CONSTRAINT stock_reserved_not_negative CHECK (reserved >= 0),
    available  integer GENERATED ALWAYS AS (on_hand - reserved) VIRTUAL,
    CONSTRAINT stock_reserved_within_on_hand CHECK (reserved <= on_hand),
    PRIMARY KEY (store_id, product_id)
);

CREATE TABLE orders (
    id          uuid         PRIMARY KEY DEFAULT uuidv7(),
    store_id    uuid         NOT NULL REFERENCES stores,
    status      order_status NOT NULL DEFAULT 'pending',
    customer    jsonb        NOT NULL,
    total_cents bigint       NOT NULL CONSTRAINT orders_total_not_negative CHECK (total_cents >= 0),
    version     integer      NOT NULL DEFAULT 1,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now()
);

CREATE INDEX orders_store_status_idx ON orders (store_id, status);
CREATE INDEX orders_created_at_idx ON orders (created_at);

CREATE TABLE order_lines (
    order_id         uuid              NOT NULL REFERENCES orders ON DELETE CASCADE,
    product_id       uuid              NOT NULL REFERENCES products,
    quantity         positive_quantity NOT NULL,
    unit_price_cents bigint            NOT NULL,
    PRIMARY KEY (order_id, product_id)
);

-- The stock ledger: one row per change to on_hand. Bulk-loaded with binary COPY by `seed`.
CREATE TABLE stock_movements (
    id         bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    store_id   uuid        NOT NULL,
    product_id uuid        NOT NULL,
    delta      integer     NOT NULL,
    reason     text        NOT NULL,
    at         timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (store_id, product_id) REFERENCES stock ON DELETE CASCADE
);

-- Work for the background worker, written in the same transaction as the change it reports
-- on, so that a job exists if and only if the change committed (the outbox pattern).
CREATE TABLE outbox (
    id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    topic        text        NOT NULL,
    payload      jsonb       NOT NULL,
    attempts     integer     NOT NULL DEFAULT 0,
    last_error   text,
    available_at timestamptz NOT NULL DEFAULT now(),
    created_at   timestamptz NOT NULL DEFAULT now(),
    done_at      timestamptz
);

CREATE INDEX outbox_pending_idx ON outbox (available_at) WHERE done_at IS NULL;
