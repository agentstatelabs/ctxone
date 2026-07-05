# Licensing & Editions

CTXone is **open source, commercially supported**. It is licensed under the
**Business Source License 1.1 (BSL 1.1)**. See [LICENSE](LICENSE) for the
authoritative license text. This document is a plain-English summary of what
that license means, plus how the **OSS / Team / Enterprise** editions fit
together.

CTXone is one half of a suite. It is the shared **team layer** for
**[AgentStateDeveloper](https://github.com/agentstatelabs/AgentStateDeveloper)**
(per-developer code context) — see [Editions](#editions) below and the READMEs
for how they pair.

## What this means for you

### You CAN (without any commercial license):

- Use CTXone in production for your own applications and services
- Use it inside your company, startup, or enterprise for internal
  operations — employees, contractors, and subsidiaries all count as
  internal
- Self-host CTXone on your own infrastructure
- Modify the source code and create derivative works for your own
  internal use
- Build applications and services that use CTXone internally as
  infrastructure — your product or service can depend on a CTXone
  Hub you run
- Use the MCP server, language bindings, CLI, Lens UI, and all
  features as part of your own business operations
- Use it for research, education, testing, and development

### You CANNOT (without a commercial license):

- **Offer CTXone itself as a commercial managed service** — e.g.,
  "CTXone-as-a-Service" where the primary value is access to
  CTXone's features
- **Embed, bundle, distribute, or sublicense CTXone as part of a
  product or service you sell, license, or distribute to third
  parties** — e.g., shipping CTXone inside your own enterprise AI
  platform that customers buy

## Editions

CTXone ships in three editions. The **OSS** edition — this repository — is
fully functional on its own, self-hosted, with no account. **Team** and
**Enterprise** are commercially supported and add collaboration and
governance on top; they don't take anything away from OSS.

| Edition | Who it's for | What it adds on top |
|---|---|---|
| **OSS** | A developer or a self-hosting team | Everything in this repo: the CTXone Hub (MCP + HTTP), the memory + graph engine, plans, recall, branches, token-savings accounting, the Lens UI, and the durability defenses. Run it on your own box; the memory db is yours. |
| **Team** | A team sharing memory | A shared team Hub as the collaboration layer — decisions, plans, and memory that travel across the whole team, with team dashboards and managed-hosting convenience. This is the edition that pairs with **[AgentStateDeveloper](https://github.com/agentstatelabs/AgentStateDeveloper)** to make "go team" mean *shared code context + shared memory*, not a lone server. |
| **Enterprise** | Organizations | Org-scale operation: multi-tenancy, RBAC / SSO, audit and compliance controls, and org-wide administration for deploying CTXone internally at scale. |

The boundaries are deliberate: **OSS is self-contained and self-hosted**;
**Team adds the shared-memory collaboration layer** (and pairs with ASD as
the suite's code-context half); **Enterprise adds multi-tenancy, access
control, and compliance.**

## Commercially supported

CTXone is built and supported commercially by **AgentStateLabs**. Team and
Enterprise editions — along with support, onboarding help, and roadmap input
— are available. The commercial offering is still being shaped, so the best
path today is to reach out and we'll work out what fits your team.

**Contact:** [licensing@agentstatelabs.com](mailto:licensing@agentstatelabs.com)

## The restrictions, explained

BSL 1.1 with this Additional Use Grant protects CTXone against two
specific patterns that would undermine the project's sustainability:

**1. Hosted resale.** A cloud provider taking the code, hosting it,
and selling "Managed CTXone" as a commercial service. This is what
BSL was originally designed to prevent — it's the pattern that led
to MongoDB, Elastic, and Redis changing their licenses after
hyperscalers strip-mined their ecosystems.

**2. Redistribution for sale.** A software vendor embedding CTXone
into a product they distribute to customers, effectively getting
commercial value from CTXone without any licensing arrangement. This
is the "we ship CTXone inside our enterprise AI platform and charge
our customers for it" pattern. Under the Additional Use Grant, this
requires a commercial license.

If you're building an application that uses CTXone internally as
infrastructure — your team runs a CTXone Hub, your application
connects to it, and you're not redistributing or reselling CTXone
itself — you're fine. That's internal business use, which is fully
permitted.

If you want to embed CTXone into a product you distribute to third
parties, resell it, or run a hosted CTXone service for customers,
you need a commercial license. Contact us at
**licensing@agentstatelabs.com**.

## Automatic conversion to Apache 2.0

Every version of CTXone automatically converts to the **Apache
License 2.0** four years after its release date:

- **v0.73.0** (released 2026-04-15) becomes Apache 2.0 on
  **2030-04-15**
- Future versions follow the same rolling per-version pattern

After conversion, all BSL restrictions lift for that version — it
becomes permissively licensed Apache 2.0. You can embed it, resell
it, ship it in your products, host it as a managed service. The
four-year clock is what keeps CTXone's ecosystem protected while
guaranteeing long-term openness.

## Why BSL?

CTXone is a new infrastructure primitive. Building and maintaining
it requires sustained investment — in the core engine, language
bindings, storage backends, MCP server, documentation, security
patches, and community support. The BSL model has been proven by
CockroachDB, Sentry, HashiCorp, MariaDB, and others to sustain
open-source infrastructure projects while preventing the
well-documented pattern of hyperscale cloud providers strip-mining
open-source value.

We chose BSL 1.1 with a redistribution-restricted Additional Use
Grant specifically because:

- It's the most battle-tested source-available license for
  infrastructure software
- The automatic Apache 2.0 conversion gives the community a
  permanent guarantee
- End users running CTXone internally are completely unaffected
- The redistribution carve-out prevents packaged resale that would
  bypass the commercial licensing we rely on for sustainability
- It's clear, readable, and has well-understood legal precedent

## Commercial licensing

If your use case requires terms beyond the BSL 1.1 grant — for
example, you want to embed CTXone into a product you distribute, or
offer a hosted CTXone service to customers — contact us at
**licensing@agentstatelabs.com** for commercial licensing options.

We offer two commercial tiers:

- **CTXone Enterprise** — for organizations deploying CTXone
  internally at scale with compliance, audit, and multi-tenancy
  requirements
- **Redistribution license** — for ISVs and product companies that
  want to embed CTXone as part of their own commercial offerings

## Questions

If you're unsure whether your use case is covered by the BSL 1.1
grant, email **licensing@agentstatelabs.com** and we'll clarify.
The bar is straightforward: internal use is free; redistribution
or hosted resale requires a commercial license.

Contact: **licensing@agentstatelabs.com**
