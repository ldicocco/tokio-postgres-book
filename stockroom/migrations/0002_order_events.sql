-- stockroom, migration 2: order events.
--
-- Every new order and every change of an order's status is announced twice, by the same
-- trigger and therefore in the same transaction as the change itself:
--
--   * on the `order_events` channel, for live screens (LISTEN/NOTIFY, Chapter 11). A
--     notification is delivered at commit, and only if the transaction commits.
--   * as a row in the outbox, for the background worker (Chapter 16), plus a payload-less
--     notification on `outbox` to wake it up. Identical notifications within one transaction
--     are delivered once, so a transaction that queues many jobs wakes the worker once.

CREATE FUNCTION announce_order_event() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    event jsonb := jsonb_build_object(
        'orderId',  NEW.id,
        'storeId',  NEW.store_id,
        'status',   NEW.status,
        'previous', CASE WHEN TG_OP = 'UPDATE' THEN OLD.status END,
        'version',  NEW.version
    );
BEGIN
    PERFORM pg_notify('order_events', event::text);
    INSERT INTO outbox (topic, payload) VALUES ('order.' || NEW.status, event);
    PERFORM pg_notify('outbox', '');
    RETURN NULL;
END
$$;

CREATE TRIGGER orders_announce_insert
    AFTER INSERT ON orders
    FOR EACH ROW EXECUTE FUNCTION announce_order_event();

CREATE TRIGGER orders_announce_status
    AFTER UPDATE OF status ON orders
    FOR EACH ROW WHEN (OLD.status IS DISTINCT FROM NEW.status)
    EXECUTE FUNCTION announce_order_event();
