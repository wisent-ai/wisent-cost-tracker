# Supabase sink: requests, reads and their refusals

The native transport reports the operation, method, URL and cause for a failed
request. A non-success HTTP response also includes its status and full response
body. Invalid JSON reports the response status and body.

Supabase reads follow server pagination rather than treating one server-sized
page as the full result. Every page is asked with `Prefer: count=exact`, and the
row total comes from the answer's `Content-Range` header (`<first>-<last>/<total>`).
An empty page before the reported total is an error, not a successful partial
read: `SupabaseSink <operation> stopped after <n> rows but Content-Range reports
<range>`.

The Python `SupabaseSink` read refuses, naming the page it was reading and the
URL, rather than returning rows whose completeness nobody measured:

| Answer | Refusal |
|---|---|
| No `Content-Range` header | `SupabaseSink <page> from <url> answered without the Content-Range header that count=exact asks for, so whether every row was read is unknown` |
| A header with no `/total` | `SupabaseSink <page> from <url> answered Content-Range <range>, which carries no row total` |
| A total that is not a whole count | `SupabaseSink <page> from <url> answered Content-Range <range>, whose row total is not a count: <cause>` |
| A page that is not a JSON array | `SupabaseSink <page> expected an array from <url>: <cause>; body: <body>` |

A proxy or gateway in front of the database that strips `Content-Range` makes
every read refuse; let the header through rather than reading without it.
