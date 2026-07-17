# stravoronoi

The aim is to generate a voronoi map based on the polylines of my previous Strava run routes.

## Need to fix.

Does the strava API guarantee the activity ID is increasing? If not do I need to add another "ID" column to the database which I can guarantee the order OR change the frontend to request using the after_epoch pattern?

Let me look at the current backend fetching logic and the run model to give an informed answer.

Now let me check the run model and the DB query ordering.

Good question, and there are actually two separate issues buried in it. Let me lay it out.

Does Strava guarantee activity IDs increase over time?
No. Strava does not document any guarantee that a newer activity has a higher id. In practice IDs have trended upward, but the API deliberately paginates by date (before/after epoch) and page/per_page — never by ID. That's a strong signal you shouldn't treat id as a time-ordered cursor.

Note that your background importer already relies on date ordering — fetch_activities pages Strava via after={epoch}, and start_fetching_runs advances using last().start_date. So runs get inserted into your DB in start_date order, which is not necessarily strava_activity_id order.

But do you need ID monotonicity for your pagination?
Your frontend↔backend paging isn't paging Strava — it's paging your local DB by primary key (get_runs filters strava_activity_id > after_id, limit 10). For that, IDs don't need to correlate with time — they just need a stable, deterministic order. So two real problems:

1. The query has no ORDER BY.

Keyset pagination is only correct if rows come back sorted by the cursor column. SQLite happens to return integer-primary-key rows in rowid order, so it works today — but that's implicit, not guaranteed by SQL. Add .order_by_asc(...) regardless.

2. Concurrency during the initial import (the real trap).

While the background fetch is still running, your frontend is paging by strava_activity_id ascending, but rows arrive in start_date order. If IDs aren't monotonic with insertion, a run can be inserted with an id behind the cursor the frontend already passed — and it'll be silently skipped until a full reload.

Recommendation
You do not need to switch the frontend to after_epoch. The cleanest fix is a dedicated auto-increment insertion-order column (a surrogate key separate from strava_activity_id), and paginate on that:

It's guaranteed unique and monotonic with insertion order, so newly-imported rows always land ahead of the cursor and can never be skipped mid-import.
It sidesteps start_date collision edge cases (two activities with the same timestamp) that an after_epoch cursor would have.
Concretely: make strava_activity_id a unique column, add an id BIGINT PRIMARY KEY AUTOINCREMENT (or a rowid surrogate), have the frontend send that id as the cursor, and add an explicit .order_by_asc(id).

Switching to after_epoch would also mostly work (inserts are start_date-ascending, so new rows stay ahead of the cursor), but it's more fragile because start_date isn't guaranteed unique.

Want me to implement the surrogate-key approach — migration/model change, get_runs cursor + ORDER BY, and the frontend next_after_id → next_id change?

