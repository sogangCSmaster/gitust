# Open-Core direction

This is a product strategy proposal. It does not define current paid editions, quotas, prices, or license exceptions.

## Community core

The intended open-source core combines local Git hosting, managed agent worktrees, CLI/MCP integration, verification, human approval, and deliberate upstream publication. Developers should be able to operate this workflow on their own machines or servers.

The current community core is licensed under [AGPL-3.0-only](../../LICENSE). Commercial use is permitted under that license, including its source-availability obligations. Future enterprise packaging must state its boundaries explicitly; this document grants no exception to the AGPL.

A separate commercial license is a possible future model, not an available offer. It would require sufficient rights from the relevant copyright holders, including contributors. Contributions currently enter under AGPL-3.0-only without copyright assignment or an additional commercial relicensing grant. Any future contributor agreement would need to be explicit before relying on it.

## Potential enterprise value

The main areas are operational needs that grow with the number of agents and machines:

1. **Central control:** administer agent permissions and external communication policies across deployments.
2. **Accountability:** trace who verified and approved a result, against which commit and policy.
3. **Fleet operation:** distribute work across on-premise machines, manage capacity, and share build caches.

Possible capabilities include enterprise identity integrations, organization-wide policies, audit reporting, stronger isolation, and cluster scheduling. These are roadmap items; existing role checks, approval records, or a local sandbox do not constitute those complete products.

## Optional compute services

An opt-in burst execution service could be explored after the local product and isolation model are established. It would require an explicit design for transferred data, secrets, encryption, execution trust, and billing. No current claim is made that remote computation can process source code without data transfer or exposure to a remote execution environment.

This service must not become a requirement for the self-hosted core. See the [vision](../vision.md) and [roadmap](../roadmap.md) for product scope.
