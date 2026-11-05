-- What-if simulation and supplier re-request plans read a decision's own dependencies (decision_id); until now
-- only the reverse directions (claim, upstream decision) were indexed, so each lookup scanned the tenant's graph.
CREATE INDEX decision_deps_of ON decision_dependencies (tenant_id, decision_id);
