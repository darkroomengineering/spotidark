# Personal Spotify app

Register your own Spotify Web API app, then connect it in **Spotidark → Settings
→ Account → Personal Spotify app**. You need a Spotify Premium account.

1. Open [developer.spotify.com/dashboard](https://developer.spotify.com/dashboard),
   sign in, and click **Create app**.
2. Choose a name that does not start with **Spot**, such as **Darkroom Desktop
   Player**. Spotify rejects app names starting with "Spot", so **Spotidark**
   is not an allowed app name. Add a description.
3. Under **Redirect URIs**, add `http://127.0.0.1:8989/login`. It must match
   exactly, including the scheme, address, port, and path. The **Copy redirect
   URI** button in Spotidark copies this value.
4. Tick **Web API**, accept the terms, and click **Save**.
5. Copy the **Client ID** from the app's page, paste it into **Personal Spotify
   app** in Spotidark Settings, and click **Authorize**. Complete Spotify's
   browser approval using the same account you use in Spotidark.

A Client ID is **32 lowercase hex characters** (`0-9` and `a-f`). Spotidark
removes whitespace around the value and disables **Authorize** until the format
is valid. Copy the Client ID, not the Client Secret. Format validation does not
prove that the app exists or that Spotify will authorize it.

## Why each developer needs their own app

Development Mode apps are limited to **5 allow-listed users**, and quota is
counted **per developer account**. Every developer needs their own app under
their own developer account. Sharing one Client ID does not give each developer
their own quota, and creating multiple apps under one account does not increase
that account's quota.

Once connected, Settings shows **Personal app ready**. Supported requests use
your app; some features still use the existing shared connection. Local playback
authorization stays separate. Select **Remove** to stop using your personal app.
