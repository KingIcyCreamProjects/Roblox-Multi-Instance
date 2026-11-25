# Roblox web API notes (verified live)

Interoperability facts checked against live responses on 2026-10-24 (no cookie unless noted). Update when Roblox changes them.

| Call | Shape (abridged) |
|---|---|
| `POST users.roblox.com/v1/usernames/users` `{usernames, excludeBannedUsers}` | `{data:[{requestedUsername, hasVerifiedBadge, id, name, displayName}]}` |
| `GET thumbnails.roblox.com/v1/users/avatar-headshot?userIds=1&size=48x48&format=Png&isCircular=false` | `{data:[{targetId, state:"Completed", imageUrl, version}]}` (imageUrl on `tr.rbxcdn.com`) |
| `GET thumbnails.roblox.com/v1/users/avatar?userIds=1&size=420x420&format=Png` | same shape |
| `POST presence.roblox.com/v1/presence/users` `{userIds}` | `{userPresences:[{userPresenceType, lastLocation, placeId, rootPlaceId, gameId, universeId, userId}]}`; `userPresenceType` 0 offline, 1 online, 2 in game, 3 in Studio; `gameId` = server JobId. Without a cookie, place/game fields are null; authenticated calls show them for users who share their presence. |
| `GET games.roblox.com/v1/games/{placeId}/servers/Public?limit=10..100&cursor=` | `{previousPageCursor, nextPageCursor, data:[{id, maxPlayers, playing, playerTokens, players, fps, ping}]}`. **`playerTokens` is empty without a cookie**, so Find-a-player must call it authenticated. |
| `GET apis.roblox.com/universes/v1/places/{placeId}/universe` | `{universeId}` |
| `GET games.roblox.com/v1/games/{placeId}/servers/VIP?limit=100&cursor=` | same shape as Public; works without a cookie (lists running private servers, no names). With a cookie it should list the owner's servers with `name`; not checked (M5 parses `name` when present, skips rows without `id`). |
| `GET games.roblox.com/v1/games?universeIds=` | `{data:[{id, rootPlaceId, name, creator:{id, name, type}, playing, maxPlayers, ...}]}` |
| `GET games.roblox.com/v1/games/votes?universeIds=` | `{data:[{id, upVotes, downVotes}]}` |
| `GET thumbnails.roblox.com/v1/games/icons?universeIds=a,b&size=150x150&format=Png&isCircular=false` | same shape as user thumbnails (`targetId` = universe) |
| `GET thumbnails.roblox.com/v1/games/multiget/thumbnails?universeIds=&countPerUniverse=1&defaults=true&size=768x432&format=Png&isCircular=false` | `{data:[{universeId, error, thumbnails:[{targetId, state, imageUrl, version}]}]}` (16:9) |
| `POST thumbnails.roblox.com/v1/batch` `[{requestId, targetId, type:"AvatarHeadShot", size:"150x150", format, isCircular}]` | `{data:[{requestId, errorCode, errorMessage, targetId, state, imageUrl, version}]}` (checked with `targetId`; the `token` form used by Find-a-player is not checked live) |
| `GET apis.roblox.com/search-api/omni-search?searchQuery=&sessionId=<uuid>&pageType=all[&pageToken=]` | `{searchResults:[{contentGroupType:"Game", contents:[{universeId, rootPlaceId, name, creatorName, playerCount, totalUpVotes, totalDownVotes, isSponsored, ...}]}], nextPageToken}` (40 games per page) |
| `GET apis.roblox.com/explore-api/v1/get-sorts?sessionId=<uuid>` | `{sorts:[{contentType:"Filters"|"Games", sortId ("top-trending", "up-and-coming", "top-playing-now", ...), sortDisplayName, games:[{universeId, rootPlaceId, name, playerCount, totalUpVotes, totalDownVotes, isSponsored, ...}]}], nextSortsPageToken}` (no creator) |
| `GET develop.roblox.com/v1/universes/{universeId}/places?sortOrder=Asc&limit=100&cursor=` | `{previousPageCursor, nextPageCursor, data:[{id, universeId, name, description}]}` |

## Used by the M4 launcher, not yet verified live

Check these in the owner-run live launch test and move them up once confirmed.

| Call | Assumed shape |
|---|---|
| `POST auth.roblox.com/v1/authentication-ticket` (cookie, `Referer: https://www.roblox.com/`, `RBXAuthenticationNegotiation: 1`, CSRF handshake) | ticket in the `rbx-authentication-ticket` response header |
| `POST apis.roblox.com/sharelinks/v1/resolve-link` `{linkId, linkType: "Server" \| "ExperienceInvite"}` (cookie) | `{privateServerInviteData: {placeId, linkCode}, experienceInviteData: {placeId}}`; on any failure the UI says "Couldn't resolve this share link" |
| `roblox-player:` URI fields and the `RobloxPlayerBeta.exe` command line carrying the tracker id | replace-previous and window-rect restore find clients by tracker id on the command line |

## Used by M5 Discover, not yet verified live

| Call | Assumed shape |
|---|---|
| `POST gamejoin.roblox.com/v1/join-game-instance` `{placeId, gameId, isTeleport:false, gameJoinAttemptId}` (cookie, CSRF handshake) | `{status, joinScript:{UdmuxEndpoints:[{Address, Port}], MachineAddress, ...}}`; region lookup takes `UdmuxEndpoints[0].Address`, else `MachineAddress`; "set server, then join" calls it once per account before launching |
| Find a player: authenticated Public pages, then `/v1/batch` with `{token, type:"AvatarHeadShot", size:"150x150"}` | the token's imageUrl equals the user's `avatar-headshot?userIds=&size=150x150` imageUrl |

## Account tools (M6)

Checked 2026-10-24 without a cookie. "Route exists" = 401 `Authentication token is missing` (or 403 `XSRF token invalid` for writes), so the path is live but the authenticated shape is **unverified** until the owner live test.

| Call | Status |
|---|---|
| `GET users.roblox.com/v1/users/{id}` | verified: `{description, created, isBanned, hasVerifiedBadge, id, name, displayName}` |
| `POST users.roblox.com/v1/users` `{userIds, excludeBannedUsers}` | verified: `{data:[{hasVerifiedBadge, id, name, displayName}]}` |
| `GET avatar.roblox.com/v2/avatar/users/{id}/outfits?outfitType=Avatar&page=1&itemsPerPage=50` | verified: `{data:[{id, name, isEditable, outfitType}], paginationToken}`. Outfit ids are ~6.5e15, still below 2^53. Next page via `&paginationToken=` is **unverified** (rate-limited while checking); we stop at 500 |
| `GET avatar.roblox.com/v1/outfits/{id}/details` | verified: `{assets:[{id, name, assetType}], bodyColors:{headColorId,…}, scale:{height,width,head,depth,proportion,bodyType}, playerAvatarType}`. v3 has `bodyColor3s` hex instead |
| `POST avatar.roblox.com/v1/avatar/set-body-colors`, `set-scales`, `set-player-avatar-type`; `POST avatar.roblox.com/v2/avatar/set-wearing-assets` `{assets:[{id}]}` | routes exist (v1 `set-wearing-assets` is 404). Reply assumed `{invalidAssetIds, success}`; unowned assets show up in `invalidAssetIds` (unverified). Steps run colors → scales → type → assets with no rollback; a failure part-way leaves earlier steps applied. Null `bodyColors`/`scale` are skipped |
| `GET thumbnails.roblox.com/v1/users/outfits?userOutfitIds=` | assumed same shape as other thumbnails (unverified) |
| `GET/POST auth.roblox.com/v1/account/pin`, `/v1/account/pin/unlock` `{pin}` | routes exist. Reply assumed `{isEnabled, unlockedUntil}`, `unlockedUntil` read as seconds left (epoch s if > 1e9, epoch ms if > 1e12) |
| PIN-locked write | assumed 400 or 403 with "PIN" in the error message → `PIN_REQUIRED` (unverified) |
| `PATCH users.roblox.com/v1/users/{id}/display-names` `{newDisplayName}` | route exists; Roblox's error message is shown as is (cooldowns) |
| `POST auth.roblox.com/v2/user/passwords/change` `{currentPassword, newPassword}` | route exists; a rotated `.ROBLOSECURITY` in `Set-Cookie` is stored |
| `GET/POST accountsettings.roblox.com/v1/email` (`{emailAddress, verified}` / `{password, emailAddress}`) | routes exist |
| Privacy | `accountsettings …/v1/privacy` and `…/private-server-invite-privacy` are **404 now**. Uses `GET apis.roblox.com/user-settings-api/v1/user-settings/settings-and-options` + `POST …/v1/user-settings` `{key: value}` (routes exist). Keys are found by substring (`joinme` → `whoCanJoinMeInExperiences`, `privateserver`); value/option fields read leniently. Shape **unverified** |
| `POST auth.roblox.com/v1/logoutfromallsessionsandreauthenticate` | route exists; new cookie taken from `Set-Cookie` |
| `POST friends.roblox.com/v1/users/{id}/request-friendship` `{friendshipOriginSourceType:"Unknown"}` | route exists; reply `{success, isCaptchaRequired}` assumed |
| Blocks | `accountsettings …/block` and `get-detailed-blocked-users` are **404 now**. Uses `POST apis.roblox.com/user-blocking-api/v1/users/{id}/block-user` / `unblock-user` and `GET …/v1/users/get-blocked-users?count=50&cursor=` (routes exist). List shape **unverified**, read leniently (`data.blockedUserIds` or id objects) |
| `GET economy.roblox.com/v1/users/{id}/currency` | route exists; `{robux}` assumed |

## Quick Log In (friend page "add account", `roblox/api/quicklogin.rs`)

| Call | Status |
|---|---|
| `POST apis.roblox.com/auth-token-service/v1/login/create` `{}` (no auth) | verified: `{code, privateKey, expirationTime}` |
| `POST apis.roblox.com/auth-token-service/v1/login/status` `{code, privateKey}` | verified: `{status: "Created" \| "UserLinked" \| "Validated" \| "Cancelled", accountName}` |
| `POST apis.roblox.com/auth-token-service/v1/login/cancel` `{code}` | route used; reply ignored (unverified) |
| `POST auth.roblox.com/v2/login` `{ctype: "AuthToken", cvalue: code, password: privateKey}` | assumed 200 + `Set-Cookie: .ROBLOSECURITY` (unverified); a 403 with `rblx-challenge-*` means an extra check (2-step verification) |
| `POST auth.roblox.com/v2/logout` (cookie, CSRF handshake) | best effort; reply ignored (unverified) |
