#!/usr/bin/env python3
"""Reproducible vulnerable WordPress plugin for the AST-graph demo. Ground truth:
2 SQLi (one unauth+interprocedural), 1 XSS (interprocedural), 1 LFI,
1 unserialize, 1 CSRF (missing nonce) — hidden among ~60 benign helper files.
Usage: make_demo_plugin.py <dir>   (default ./acme-plugin)
"""
import os, random, sys
root = sys.argv[1] if len(sys.argv) > 1 else "./acme-plugin"
inc = os.path.join(root, "includes"); os.makedirs(inc, exist_ok=True); random.seed(3)
FILES = {
 "acme.php": """<?php
/* Plugin Name: Acme Tools */
add_action('wp_ajax_acme_search',        'acme_ajax_search');
add_action('wp_ajax_nopriv_acme_search', 'acme_ajax_search');
add_action('wp_ajax_acme_delete',        'acme_delete_item');
add_action('admin_post_acme_save',       'acme_save_settings');
add_action('init',                       'acme_show_widget');
add_action('wp_ajax_acme_import',        'acme_import_data');
""",
 "includes/search.php": """<?php
function acme_ajax_search() {
    $term = $_GET['term'];              // taint source
    return acme_find_items($term);      // flows to db.php
}
""",
 "includes/db.php": """<?php
function acme_find_items($needle) {
    global $wpdb;
    return $wpdb->get_results("SELECT * FROM {$wpdb->prefix}items WHERE name = '$needle'");
}
function acme_delete_item() {
    global $wpdb;
    $wpdb->query("DELETE FROM {$wpdb->prefix}items WHERE id = " . $_POST['id']);
}
""",
 "includes/render.php": """<?php
function acme_show_widget() {
    $title = $_REQUEST['title'];        // taint source
    acme_render_box($title);
}
function acme_render_box($text) {
    echo "<div class='acme-box'>" . $text . "</div>";   // xss sink
}
""",
 "includes/admin.php": """<?php
function acme_save_settings() {
    update_option('acme_opts', $_POST['opts']);   // state change, no nonce -> CSRF
}
function acme_load_template() {
    include($_GET['tpl'] . '.php');                // LFI
}
""",
 "includes/importer.php": """<?php
function acme_import_data() {
    return unserialize($_POST['payload']);         // object injection
}
""",
}
for rel, code in FILES.items():
    open(os.path.join(root, rel), "w").write(code)
for i in range(60):
    fns = "\n".join(
        f"function acme_util_{i}_{j}($x) {{\n    return esc_html(sanitize_text_field($x));\n}}"
        for j in range(random.randint(2, 5)))
    open(os.path.join(inc, f"util_{i:02d}.php"), "w").write("<?php\n" + fns + "\n")
n = sum(len(f) for _, _, f in os.walk(root))
print(f"wrote {n} files to {root}/  (6 ground-truth vulnerabilities among ~60 benign files)")
