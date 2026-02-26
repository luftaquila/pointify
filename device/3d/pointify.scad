// ==========================================
// 1. Configuration & Parameters
// ==========================================
$fn = 100;

// [Grid Configuration]
num_cols = 3; // Columns (Width)
num_rows = 2; // Rows (Height)

// [Dimensions]
hole_sz = 46; // Unit hole size
hole_w = hole_sz - 1.0; // Hole width
hole_h = hole_sz - 0.5; // Hole height

gap = 2.0; // Wall thickness
margin_bot = 10; // Bottom margin
margin_top = 2.0; // Top margin

// [Calculated Dimensions]
// Total Width = (Cols * Hole) + Walls
face_w = (num_cols * hole_w) + ( (num_cols + 1) * gap);
// Total Height = (Rows * Hole) + Walls + Margins
face_h = margin_bot + (num_rows * hole_h) + ( (num_rows - 1) * gap) + margin_top;

// [View Settings]
explode_dist = 40; // 0 for assembly, >0 for exploded view
tolerance = 0.2;

// [Cover & Screw Settings]
cover_thickness = 3.0;
m2_tap_radius = 1.0; // Body hole (M2 Tap)
m2_clearance_radius = 1.5; // Cover hole (M2 Clearance)
m2_head_radius = 2.1; // Counterbore dia
m2_head_depth = 2.0; // Counterbore depth

// [Mounting Tab Settings]
tab_depth_len = 4;
tab_fillet_r = 4.0;

// [PCB & USB Settings]
pcb_hole_dist = 17.15;
pcb_dist_from_cover = 3.8;
pcb_boss_height = 5.0;
pcb_boss_dia = 5.0;
patch_nut_height = 2.0;

usb_w = 9.8;
usb_h = 3.8;
usb_r = usb_h / 2;

// [Geometry Definition]
tilt_angle = 20;
top_side_length = 32;
tab_depth = 9.5;
floor_thickness = 2.0;

// [Front Tab Settings]
hole_x = 6;
hole_z = 6;
target_diag_dist = 11;
tab_size_lower = 9.2;
tab_size_upper = tab_size_lower * 0.7;
tab_thick_lower = 5.0;
tab_thick_upper = 3.0;
hole_tap_radius = 1.7;

divider_cut_depth = tab_depth + tab_thick_lower;

// [Anti-slip Sticker Recess]
sticker_dia = 12.0;
sticker_depth = 1.0;
sticker_inset_x = 12;
sticker_inset_y = 12;

// ==========================================
// 2. Main Assembly
// ==========================================

union() {
  difference() {
    union() {
      // Main Body with Internal Cutouts
      difference() {
        shell_body();
        intersection() {
          rotated_holes_with_cut_dividers();
          translate([-50, -200, floor_thickness])
            cube([face_w + 100, 500, 300]);
        }
      }
      // Add Components
      rotated_tabs();
      rear_mounting_tabs_fixed_height();
      pcb_bosses_solid();
    }
    // Subtract PCB Screw Holes
    pcb_boss_holes();
    // Subtract Anti-slip Sticker Recesses
    sticker_recesses();
  }
}

// Back Cover (Positioned based on explode_dist)
translate([0, calc_back_cut_y() + explode_dist, 0])
  lip_insert_counterbored();

// ==========================================
// 3. Modules & Functions
// ==========================================

// Module: Back Cover with USB & Screw Holes
module lip_insert_counterbored() {
  y_cut = calc_back_cut_y();
  h_top_local = face_h - margin_top;
  h_bot_local = margin_bot;

  z_top_raw = ( -y_cut * tan(tilt_angle)) + (h_top_local / cos(tilt_angle));
  z_bot_raw = ( -y_cut * tan(tilt_angle)) + (h_bot_local / cos(tilt_angle));
  z_bot_actual = max(z_bot_raw, floor_thickness);
  lip_h = z_top_raw - z_bot_actual;

  inner_total_width = face_w - (2 * gap);
  margin_x = gap + 4;
  end_x = gap + inner_total_width;

  // Top Tabs Z reference
  h_tabs_top_local = face_h - margin_top;
  z_top_tabs = ( -y_cut * tan(tilt_angle)) + (h_tabs_top_local / cos(tilt_angle));

  z_top_hole = z_top_tabs - 4;
  z_bot_hole = z_bot_actual + 4;
  hole_coords = [[margin_x, z_top_hole], [end_x - 4, z_top_hole], [margin_x, z_bot_hole], [end_x - 4, z_bot_hole]];

  // USB Position Calculation
  div_center_x = gap + hole_w + (gap / 2);
  pcb_bottom_z = floor_thickness + pcb_boss_height + patch_nut_height;
  usb_center_z = pcb_bottom_z - 1.7; // USB connector center (fixed)

  difference() {
    // Main Lip Body
    color("#ffaa00")
      translate([gap + tolerance, -cover_thickness, z_bot_actual + tolerance])
        cube([inner_total_width - (2 * tolerance), cover_thickness, lip_h - (2 * tolerance)]);

    // Screw Holes
    for (pt = hole_coords) {
      translate([pt[0], 0, pt[1]]) rotate([90, 0, 0])
          union() {
            translate([0, 0, -1]) cylinder(r=m2_clearance_radius, h=cover_thickness + 5);
            translate([0, 0, -0.1]) cylinder(r=m2_head_radius, h=m2_head_depth + 0.1);
          }
    }

    // USB Type-C Cutout (Racetrack)
    translate([div_center_x, 1, usb_center_z])
      rotate([90, 0, 0])
        linear_extrude(height=cover_thickness + 5) {
          hull() {
            translate([-(usb_w / 2 - usb_r), 0]) circle(r=usb_r);
            translate([(usb_w / 2 - usb_r), 0]) circle(r=usb_r);
          }
        }
  }
}

// Module: PCB Mounting Bosses (Solid)
module pcb_bosses_solid() {
  y_cut = calc_back_cut_y();
  y_pos = y_cut - cover_thickness - pcb_dist_from_cover;
  div_center_x = gap + hole_w + (gap / 2);
  x_pos_left = div_center_x - (pcb_hole_dist / 2);
  x_pos_right = div_center_x + (pcb_hole_dist / 2);

  translate([0, 0, floor_thickness]) {
    translate([x_pos_left, y_pos, 0]) cylinder(d=pcb_boss_dia, h=pcb_boss_height);
    translate([x_pos_right, y_pos, 0]) cylinder(d=pcb_boss_dia, h=pcb_boss_height);
  }
}

// Module: PCB Mounting Holes (Counterbored)
module pcb_boss_holes() {
  y_cut = calc_back_cut_y();
  y_pos = y_cut - cover_thickness - pcb_dist_from_cover;
  div_center_x = gap + hole_w + (gap / 2);
  x_pos_left = div_center_x - (pcb_hole_dist / 2);
  x_pos_right = div_center_x + (pcb_hole_dist / 2);

  for (x = [x_pos_left, x_pos_right]) {
    translate([x, y_pos, -0.1]) {
      union() {
        cylinder(r=m2_head_radius, h=floor_thickness + 0.1);
        translate([0, 0, floor_thickness]) cylinder(r=m2_clearance_radius, h=pcb_boss_height + 10);
      }
    }
  }
}

// Module: Rear Mounting Tabs (Auto-sizing)
module rear_mounting_tabs_fixed_height() {
  y_cut = calc_back_cut_y();
  h_top_local = face_h - margin_top;
  h_bot_local = margin_bot;

  z_top_global = ( -y_cut * tan(tilt_angle)) + (h_top_local / cos(tilt_angle));
  z_bot_raw = ( -y_cut * tan(tilt_angle)) + (h_bot_local / cos(tilt_angle));
  z_bot_global = max(z_bot_raw, floor_thickness);

  inner_total_width = face_w - (2 * gap);
  start_x = gap;
  end_x = gap + inner_total_width;

  tab_w = 8;
  tab_h = 8;
  margin_x = gap + 4;

  z_top_hole = z_top_global - 4;
  z_bot_hole = z_bot_global + 4;
  hole_coords = [[margin_x, z_top_hole], [end_x - 4, z_top_hole], [margin_x, z_bot_hole], [end_x - 4, z_bot_hole]];

  translate([0, y_cut, 0]) difference() {
      union() {
        // Bottom Tabs
        translate([start_x, -cover_thickness - tab_depth_len, z_bot_global])
          block_y_fillet_top(tab_w, tab_depth_len, tab_h, tab_fillet_r);
        translate([end_x, -cover_thickness - tab_depth_len, z_bot_global])
          mirror([1, 0, 0]) block_y_fillet_top(tab_w, tab_depth_len, tab_h, tab_fillet_r);

        // Top Tabs (Intersect with Ceiling)
        intersection() {
          union() {
            fixed_bottom_z = z_top_global - tab_h;
            translate([start_x, -cover_thickness - tab_depth_len, fixed_bottom_z])
              block_y_fillet_bottom(tab_w, tab_depth_len, tab_h + 15, tab_fillet_r);
            translate([end_x, -cover_thickness - tab_depth_len, fixed_bottom_z])
              mirror([1, 0, 0]) block_y_fillet_bottom(tab_w, tab_depth_len, tab_h + 15, tab_fillet_r);
          }
          translate([0, -y_cut, 0]) rotate([-tilt_angle, 0, 0])
              translate([-100, -200, -100]) cube([face_w + 100, 500, 100 + face_h]);
        }
      }
      // Screw Holes
      for (pt = hole_coords) {
        translate([pt[0], -cover_thickness + 1, pt[1]]) rotate([90, 0, 0])
            cylinder(r=m2_tap_radius, h=tab_depth_len + 5);
      }
    }
}

// Module: Internal Grid Cutouts
module rotated_holes_with_cut_dividers() {
  rotate([-tilt_angle, 0, 0]) union() {
      for (r = [0:num_rows - 1]) {
        for (c = [0:num_cols - 1]) {
          translate([gap + c * (hole_w + gap), -1, margin_bot + r * (hole_h + gap)])
            cube([hole_w, 150, hole_h]);
        }
      }
      // Rear Cutout (Unified Chamber)
      inner_total_width = face_w - (2 * gap);
      total_grid_h = (num_rows * hole_h) + ((num_rows - 1) * gap);
      translate([gap, divider_cut_depth, margin_bot])
        cube([inner_total_width, 150, total_grid_h]);
    }
}

// Module: Front Mounting Tabs (Loop)
module rotated_tabs() {
  rotate([-tilt_angle, 0, 0])for (r = [0:num_rows - 1]) {
    for (c = [0:num_cols - 1]) {
      translate([gap + c * (hole_w + gap) + (hole_w - hole_sz) / 2, tab_depth, margin_bot + r * (hole_h + gap) + (hole_h - hole_sz) / 2])
        render_single_tab_set();
    }
  }
}

// Helper: Tab Geometry (Upper/Lower Blocks with Fillets)
module block_y_fillet_bottom(w, d, h, r) {
  translate([0, d, 0]) rotate([90, 0, 0]) linear_extrude(height=d) {
        hull() { square([0.1, 0.1]); translate([w - r, r]) circle(r=r); translate([0, h - 0.1]) square([0.1, 0.1]); translate([w - 0.1, h - 0.1]) square([0.1, 0.1]); }
      }
}
module block_y_fillet_top(w, d, h, r) {
  translate([0, d, 0]) rotate([90, 0, 0]) linear_extrude(height=d) {
        hull() { square([0.1, 0.1]); translate([w - 0.1, 0]) square([0.1, 0.1]); translate([0, h - 0.1]) square([0.1, 0.1]); translate([w - r, h - r]) circle(r=r); }
      }
}

// Helper: Main Shell Body
module shell_body() {
  difference() {
    rotate([-tilt_angle, 0, 0]) union() {
        cube([face_w, 100, face_h]);
        translate([0, 0, -50]) cube([face_w, 100, 50]);
      }
    translate([-50, calc_back_cut_y(), -200]) cube([face_w + 100, 200, 400]);
    translate([-50, -100, -100]) cube([face_w + 100, 300, 100]);
  }
}

// Helper: Front Tab Set (4 Tabs per Hole)
module render_single_tab_set() {
  translate([0, 0, 0]) tab_shape_variable(tab_size_lower, tab_thick_lower, hole=true);
  translate([hole_sz, 0, 0]) mirror([1, 0, 0]) tab_shape_variable(tab_size_lower, tab_thick_lower, hole=true);
  translate([0, 0, hole_sz]) mirror([0, 0, 1]) tab_shape_variable(tab_size_upper, tab_thick_upper, hole=false);
  translate([hole_sz, 0, hole_sz]) mirror([1, 0, 0]) mirror([0, 0, 1]) tab_shape_variable(tab_size_upper, tab_thick_upper, hole=false);
}

// Helper: Individual Tab Shape
module tab_shape_variable(size, thick, hole) {
  diag_natural = size * sqrt(2);
  fillet_r = (diag_natural > target_diag_dist) ? (size * sqrt(2) - target_diag_dist) / (sqrt(2) - 1) : 1.0;
  difference() {
    hull() {
      cube([size - fillet_r, thick, size]);
      cube([size, thick, size - fillet_r]);
      translate([size - fillet_r, 0, size - fillet_r]) rotate([-90, 0, 0]) cylinder(r=fillet_r, h=thick);
    }
    if (hole) {
      translate([hole_x, -1, hole_z]) rotate([-90, 0, 0]) cylinder(h=thick + 5, r=hole_tap_radius);
    }
  }
}

// Helper: Anti-slip Sticker Recesses
module sticker_recesses() {
  y_back = calc_back_cut_y();
  positions = [
    [sticker_inset_x, sticker_inset_y],
    [face_w - sticker_inset_x, sticker_inset_y],
    [sticker_inset_x, y_back - sticker_inset_y],
    [face_w - sticker_inset_x, y_back - sticker_inset_y]
  ];
  for (pos = positions) {
    translate([pos[0], pos[1], -0.01])
      cylinder(d=sticker_dia, h=sticker_depth + 0.01);
  }
}

// Helper: Back Cut Y Calculation
function calc_back_cut_y() = (top_side_length * cos(tilt_angle)) + (face_h * sin(tilt_angle));
