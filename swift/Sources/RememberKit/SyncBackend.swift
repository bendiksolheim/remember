/// The Supabase project both apps sync through, so the Mac and iOS can't
/// drift apart. Bare project URL, no path — `remember-sync`'s
/// `HttpTransport`/`AuthClient` append `/rest/v1/...` and `/auth/v1/...`
/// themselves. The key is the publishable (anon) one, meant to ship in apps.
public enum SyncBackend {
    public static let url = "https://xzmbkeeaycyaluadzxhe.supabase.co"
    public static let anonKey = "sb_publishable_xRie_YdgGu7lS0ozQzYeUg_Ww0Ek7mE"
}
