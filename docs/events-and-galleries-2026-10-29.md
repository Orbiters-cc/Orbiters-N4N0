# Events and community galleries (2026-10-29)

Enzo asked for Orbiters' events and the galleries of the servers the member is in inside Orbiters - N4N0, "as much
inspiration as you can from the website".

## Events (sidebar › Events, `pages/events.rs`)

- **What is listed**: what the website's homepage lists (`GET /community-events/upcoming`, published public events of
  organisers in good standing) plus, with `live=true`, the events under way (the homepage leaves those out). Each item
  carries what its card shows (cover flag, host communities, the main world when VRChat already named it, the number of
  steps; server `services/events/eventCards.js`, shared with the event card above a gallery).
- **Page**: a static header (search over titles, descriptions, hosts and worlds; Refresh), the next event as a hero
  (its cover with the cover's own light around it, the website's status pill, a 2-line title, when, host, world and
  Join), then cards in sections, Happening now, This week and Later (the cards' date tiles carry their day, so wide
  windows fill their rows instead of one short row a day): cover with status (a countdown with a green dot, Happening
  now in red with a pulse) and category, the homepage's date tile, title, times, host and world, Join (On the list once
  the member signed up from its window). Times are in this computer's time zone (shown in the header, e.g. UTC+1); countdowns refresh every 15 s.
- **The event's window** (`events::Window`, `orbiters_ui::viewer::Morph`): Join (or a press anywhere on the card) grows
  the card's cover into the window's hero on the picture window's spring and lands back into it with the bounce. What
  the card knew shows at once; the event's page (`/community-events/public/:id`), the member's invites
  (`/:id/invites/me`) and the event's pictures arrive behind it (an outline meanwhile). Hero: cover (large version
  fading in over the card's), category, status, title, when "· your time (UTC+1)". Body: host with Hosted by, the
  actions (Join in VRChat while an instance is open, Invite me, Open on Orbiters), the facts as pills (category, who
  may come, steps, 18+, Also on Discord), the description (Markdown as plain text), the program as the website's step
  rows (time pill, world picture, title and world, Live, its own Join and Invite me), Pictures from this event (its
  gallery's newest; a press opens the picture window over the event's).
- **Invite me** is the website's step sign-up (`POST/DELETE /community-events/:id/steps/:step/invite`): the community's
  VRChat account invites the member when the step starts and its instance is ready. The control morphs between its
  states (width on a spring, the new face fading in): Invite me → Signing you up… → Will invite you + Cancel, Invite
  sent / Invite on its way. What is missing is said under the actions with its way out: VRChat not linked (Connect on
  Orbiters opens the website's invite page, which links it; Check again), not in the group or without its role (Open
  the group, Check again = eligibility with `refresh=true`, then the sign-up), the community's account not a friend
  (Open the account), a failed invite and why (`not_friends`, `not_member`), signed out (Sign in). For a step whose
  instance is open but takes no sign-ups, Invite me asks Orbiters' own VRChat account to invite the member there
  (`POST /orb/invites`, Home's Previous instances), with its own "add Orbiters as a friend" answer. A dead Invite me
  (nothing open, over, loading) stays a quiet button with the reason as its tooltip.

## Communities in Photos & captures (`pages/community.rs`, hooks in `pages/gallery.rs`)

- A fourth filter, **Communities**, beside All, Mine and Shared with me; its gallery chips (All galleries, then the
  member's own, their communities', the public ones, as the website orders them; event galleries with a calendar).
  All shows the communities' pictures among the member's own, newest first (`GET /galleries/images?sort=date`).
- Groups are by gallery and day. A gallery's heading is its name, its audience (Members, Public, Yours) and when;
  an **event's gallery heads with the event's card** (the website's GalleryEventCard: cover, Event and its status,
  title, when, world, host, Event details), which grows into the same event window (without its own pictures strip).
- Cards are the gallery's card: the picture with its audience (or Event) and its likes, the author's Discord picture
  and name, the time, then the Discord message's first line (mentions as names) or its gallery.
- A picture opens in the **picture window's look and motion** (`community::PictureWindow`: grown out of its card on
  `Spring::EXPAND`, sized to the picture, wide ones over their details, tall ones beside, ←/→ and the round arrows
  through the shown community pictures, the next sliding in): who posted it and when, Open on Orbiters
  (`/gallery?tab=gallery-<id>`) and the full picture, its gallery, the message, likes and replies, and Taken at <event>,
  which opens the event's window out of that line.
- Search finds galleries, events, authors and words; Show more pages both feeds.

## Server (dev branch)

`22c97f69`: `upcoming?live=true` and the card fields (tests: upcomingEvents), and the app's credential (`orbit-…`) on
`/community-events/(upcoming|public)…`, `/:id/invites/me`, `/:id/steps/:step/(eligibility|invite)` (these four now
behind `requireAuth` instead of the website-only JWT strategy) and the gallery reads (`/galleries`, `/galleries/images`,
`/galleries/:id`, `/galleries/:id/images`, `/galleries/images/:id`); never event management or hiding pictures (tests:
authUtils, eventInviteToolRoutes). `8abff759`: the invite routes keep their refusal codes (`VRCHAT_LINK_REQUIRED`,
`VRCHAT_GROUP_REQUIRED`; the shared handler dropped them, so the website's step button never sent members to link
VRChat either). The desktop client is `orbiters_core::community` (fixtures in
`crates/orbiters-core/tests/fixtures/{events,event-overview,galleries,gallery-pictures}.json`).

## Screenshots

`orbiters-desktop --screenshot <dir> --only events` renders every state below (fixtures; times moved so the first event
is under way and the second starts in 2 h 14 min; this computer's captures stand in for covers and pictures):
`events`, `events-enter-mid`, `events-empty`, `events-error`, `events-search`, `events-open` (from the hero),
`events-open-mid`, `events-open-card` (an upcoming event: no instance, Invite me green), `events-close-mid`,
`events-loading`, `events-invite-busy|listed|friend|group|link|sent|orb`, `events-pictures`, and in Photos & captures
`gallery-community`, `gallery-community-all`, `gallery-community-one` (an event gallery's chip),
`gallery-community-open(-mid)` (a picture), `gallery-community-event(-mid)` (the event's card into its window).

## Checked against a disposable Orbiters

A throwaway backend (its own Postgres) seeded with a member and their app credential, a VRChat group and Discord
server the organiser registered, a public event under way with two steps and its gallery with two pictures, and an
upcoming event: the homepage list still holds the upcoming one only, `live=true` both (the live one first) with their
cards; the event page, invites/me, eligibility (`refresh=true`), Invite me (409 `VRCHAT_LINK_REQUIRED` for a member
without VRChat linked), its withdrawal and the gallery reads answer the app credential; hiding a picture, the
organiser's event list and the audience refuse it (401). `orbiters_core::community`'s ignored test runs the app's own
client against it (`ORBITERS_TEST_API`, `ORBITERS_TEST_TOKEN`, `ORBITERS_TEST_EVENT`). An event is listed only while
its organiser backs both of its destinations (eventOrganiserStanding.js): a seed without the Discord server lists
nothing.

## Not verified here

Nothing was tried against the production or development servers or in VRChat: the fixtures stand for real events in
the screenshots. Invite me's real flow (eligibility, VRChat link, group roles, the community account's friendship,
the invite worker) and Orbiters' own invite into an open instance were not exercised; covers and gallery pictures were
not fetched from Orbiters or Discord's CDN.
