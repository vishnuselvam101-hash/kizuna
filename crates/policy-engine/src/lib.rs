//! Deny-by-default authorization. A permission is allowed only when every check passes.

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    TenantAdmin,
    Reviewer,
    Analyst,
    Auditor,
    Viewer,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown role")]
pub struct UnknownRole;

impl Role {
    pub const ALL: [Role; 5] = [
        Role::TenantAdmin,
        Role::Reviewer,
        Role::Analyst,
        Role::Auditor,
        Role::Viewer,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::TenantAdmin => "tenant_admin",
            Role::Reviewer => "reviewer",
            Role::Analyst => "analyst",
            Role::Auditor => "auditor",
            Role::Viewer => "viewer",
        }
    }

    pub fn parse(s: &str) -> Result<Role, UnknownRole> {
        Role::ALL.into_iter().find(|r| r.as_str() == s).ok_or(UnknownRole)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    UserRead,
    UserManage,
    AuditRead,
    AuditVerify,
    SecurityEventRead,
    ArtifactRead,
    ArtifactUpload,
    ClaimRead,
    ClaimWrite,
    ReviewDecide,
    DecisionRead,
    DecisionWrite,
    IncidentManage,
}

impl Permission {
    /// Privileged permissions need an MFA-verified session. Review decisions (waivers, rejections, reconfirmations,
    /// entity merges) are privileged since review finding L6.
    pub fn is_privileged(self) -> bool {
        matches!(
            self,
            Permission::UserManage | Permission::IncidentManage | Permission::ReviewDecide
        )
    }
}

/// Explicit grant table. Anything not listed is denied.
pub fn role_grants(role: Role, p: Permission) -> bool {
    use Permission::*;
    match role {
        Role::TenantAdmin => true,
        Role::Auditor => matches!(
            p,
            UserRead | AuditRead | AuditVerify | SecurityEventRead | ArtifactRead | ClaimRead | DecisionRead
        ),
        Role::Reviewer => {
            matches!(
                p,
                ArtifactRead | ArtifactUpload | ClaimRead | ClaimWrite | ReviewDecide | DecisionRead | DecisionWrite
            )
        }
        Role::Analyst => matches!(p, ArtifactRead | ArtifactUpload | ClaimRead | ClaimWrite | DecisionRead),
        Role::Viewer => matches!(p, ArtifactRead | ClaimRead | DecisionRead),
    }
}

#[derive(Debug, Clone)]
pub struct Subject {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub role: Role,
    pub mfa_verified: bool,
}

/// Attributes of the resource being accessed.
#[derive(Debug, Clone)]
pub struct Resource<'a> {
    pub tenant_id: Uuid,
    pub kind: &'a str,
    pub owner_user_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(DenyReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    RoleLacksPermission,
    CrossTenant,
    MfaRequired,
    AttributeRule(&'static str),
}

impl DenyReason {
    pub fn code(self) -> &'static str {
        match self {
            DenyReason::RoleLacksPermission => "role_lacks_permission",
            DenyReason::CrossTenant => "cross_tenant",
            DenyReason::MfaRequired => "mfa_required",
            DenyReason::AttributeRule(n) => n,
        }
    }
}

/// ABAC hook: extra rules evaluated after RBAC. Returning `Some(name)` denies.
pub trait AttributeRule: Send + Sync {
    fn deny(&self, s: &Subject, p: Permission, r: &Resource<'_>) -> Option<&'static str>;
}

#[derive(Default)]
pub struct Policy {
    rules: Vec<Box<dyn AttributeRule>>,
}

impl Policy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_rule(mut self, r: impl AttributeRule + 'static) -> Self {
        self.rules.push(Box::new(r));
        self
    }

    pub fn authorize(&self, s: &Subject, p: Permission, r: &Resource<'_>) -> Decision {
        if r.tenant_id != s.tenant_id {
            return Decision::Deny(DenyReason::CrossTenant);
        }
        if !role_grants(s.role, p) {
            return Decision::Deny(DenyReason::RoleLacksPermission);
        }
        if p.is_privileged() && !s.mfa_verified {
            return Decision::Deny(DenyReason::MfaRequired);
        }
        for rule in &self.rules {
            if let Some(n) = rule.deny(s, p, r) {
                return Decision::Deny(DenyReason::AttributeRule(n));
            }
        }
        Decision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Permission::*;

    const ALL_P: [Permission; 13] = [
        UserRead,
        UserManage,
        AuditRead,
        AuditVerify,
        SecurityEventRead,
        ArtifactRead,
        ArtifactUpload,
        ClaimRead,
        ClaimWrite,
        ReviewDecide,
        DecisionRead,
        DecisionWrite,
        IncidentManage,
    ];

    fn subj(role: Role, mfa: bool) -> Subject {
        Subject {
            tenant_id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            role,
            mfa_verified: mfa,
        }
    }
    fn res(t: u128) -> Resource<'static> {
        Resource {
            tenant_id: Uuid::from_u128(t),
            kind: "x",
            owner_user_id: None,
        }
    }

    #[test]
    fn role_parse_roundtrip_and_unknown_denied() {
        for r in Role::ALL {
            assert_eq!(Role::parse(r.as_str()), Ok(r));
        }
        assert_eq!(Role::parse("TENANT_ADMIN"), Err(UnknownRole));
        assert_eq!(Role::parse("superuser"), Err(UnknownRole));
    }

    #[test]
    fn cross_tenant_always_denied() {
        let p = Policy::new();
        for r in Role::ALL {
            for perm in ALL_P {
                assert_eq!(
                    p.authorize(&subj(r, true), perm, &res(99)),
                    Decision::Deny(DenyReason::CrossTenant)
                );
            }
        }
    }

    #[test]
    fn privileged_needs_mfa() {
        let p = Policy::new();
        assert_eq!(
            p.authorize(&subj(Role::TenantAdmin, false), UserManage, &res(1)),
            Decision::Deny(DenyReason::MfaRequired)
        );
        assert_eq!(
            p.authorize(&subj(Role::TenantAdmin, true), UserManage, &res(1)),
            Decision::Allow
        );
    }

    /// Mutation-testing kills: review decisions need MFA, and the auditor role is strictly read-only.
    #[test]
    fn review_decisions_are_privileged() {
        let p = Policy::new();
        assert_eq!(
            p.authorize(&subj(Role::Reviewer, false), ReviewDecide, &res(1)),
            Decision::Deny(DenyReason::MfaRequired)
        );
        assert_eq!(
            p.authorize(&subj(Role::Reviewer, true), ReviewDecide, &res(1)),
            Decision::Allow
        );
    }

    #[test]
    fn auditor_grants_are_exactly_read_only() {
        let granted: Vec<Permission> = ALL_P.into_iter().filter(|x| role_grants(Role::Auditor, *x)).collect();
        assert_eq!(
            granted,
            vec![
                UserRead,
                AuditRead,
                AuditVerify,
                SecurityEventRead,
                ArtifactRead,
                ClaimRead,
                DecisionRead
            ]
        );
    }

    #[test]
    fn only_admin_manages_users_and_incidents() {
        let p = Policy::new();
        for r in Role::ALL.into_iter().filter(|r| *r != Role::TenantAdmin) {
            for perm in [UserManage, IncidentManage] {
                assert_eq!(
                    p.authorize(&subj(r, true), perm, &res(1)),
                    Decision::Deny(DenyReason::RoleLacksPermission)
                );
            }
        }
        assert_eq!(
            p.authorize(&subj(Role::Viewer, true), ClaimWrite, &res(1)),
            Decision::Deny(DenyReason::RoleLacksPermission)
        );
        assert_eq!(
            p.authorize(&subj(Role::Analyst, true), ReviewDecide, &res(1)),
            Decision::Deny(DenyReason::RoleLacksPermission)
        );
    }

    struct NoSelfReview;
    impl AttributeRule for NoSelfReview {
        fn deny(&self, s: &Subject, p: Permission, r: &Resource<'_>) -> Option<&'static str> {
            (p == ReviewDecide && r.owner_user_id == Some(s.user_id)).then_some("no_self_review")
        }
    }

    #[test]
    fn attribute_rule_hook_denies() {
        let p = Policy::new().with_rule(NoSelfReview);
        let s = subj(Role::Reviewer, true);
        let own = Resource {
            tenant_id: s.tenant_id,
            kind: "claim",
            owner_user_id: Some(s.user_id),
        };
        assert_eq!(
            p.authorize(&s, ReviewDecide, &own),
            Decision::Deny(DenyReason::AttributeRule("no_self_review"))
        );
    }
}
