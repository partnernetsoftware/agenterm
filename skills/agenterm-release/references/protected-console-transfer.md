# Protected console-to-GitHub transfer

Use this procedure when a release credential already exists in a provider
console, the account holder has opened an authenticated browser session, and a
single protected value must be transferred into a GitHub Environment. This is
an enrollment or recovery procedure, never part of Candidate or Promotion.

## Invariants

- Prefer AgenTerm's own `agenterm-cu` browser observation surface. A successful
  typed CU reply is stronger evidence than ad-hoc AppleScript, clipboard
  scraping, screenshots, or a raw DevTools HTTP probe.
- Grant observation only, select the exact browser target, and read the
  smallest bounded page representation that contains the field.
- Keep the value in one process pipeline: parse, validate its expected shape,
  and feed it directly to `gh secret set`. Never print it, save it to a file,
  copy it to the clipboard, paste it into a handoff, or include it in a receipt.
- Verify only the destination secret **name** and update timestamp. GitHub does
  not reveal Environment secret values, and the operator must not manufacture a
  second copy merely to compare it.
- Record durable facts only: repository, Environment, configured name, reviewer
  boundary, source identity reused, and whether a replacement credential was
  created. Never record the value.

## Procedure

1. Confirm the browser is the dedicated company-management profile, the URL is
   the expected provider domain, and the visible organization/account is the
   intended one. Stop if any of those identities are ambiguous.
2. Build the repository-owned CU client only when no compatible binary is
   already available:

   ```sh
   cargo build -p agenterm-cu
   ```

3. Discover or read through the typed CU door. For a page already selected in
   the browser, the usual bounded observation is:

   ```sh
   target/debug/agenterm-cu --target current --grant observe \
     page-text --port 9222 --target-url <PROVIDER_DOMAIN> --max-bytes 65536
   ```

   Use `page-targets` or `page-find` first if the target is not unique. Do not
   broaden the target selector merely to make the command succeed.
4. Pipe the JSON reply to a local parser. Select the field by its adjacent
   accessible label, require exactly one match, validate the provider-specific
   shape, and write the value directly to GitHub:

   ```sh
   target/debug/agenterm-cu ... page-text ... \
     | <LOCAL_PARSER_AND_VALIDATOR> \
     | gh secret set <SECRET_NAME> --repo <OWNER/REPOSITORY> \
         --env release-signing
   ```

   The parser must fail closed on zero matches, multiple matches, truncation,
   malformed JSON, an unexpected target URL, or an invalid value shape.
5. List the Environment's secret metadata through GitHub and check that the
   expected name exists. Do not ask GitHub to echo a value; it cannot and should
   not.
6. Close any provider page that exposes sensitive identifiers when the transfer
   is complete. Preserve the source credential in the company vault. Do not
   issue a new certificate or API key solely because one metadata field was
   missing from GitHub.

## Operational lessons

- A browser's raw DevTools discovery endpoint may reject a request even while
  the repository-owned CU client can read the page through its typed CDP and
  accessibility path. Treat the CU result, not a generic `/json/list` probe, as
  the product-level observation contract.
- Native Save panels, keychain prompts, and one-time-download confirmations are
  OS windows rather than web content. They require the native CU target or the
  account holder; a page-text success does not prove that a downloaded key was
  saved.
- Never make release qualification depend on scraping a provider console.
  Candidate consumes preconfigured protected secrets; it does not log in to
  Apple, Azure, or GitHub settings pages.
- Public Promotion remains a separate human authority boundary. Successful
  credential enrollment grants no tag or Release authority.
