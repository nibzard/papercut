// ABOUTME: p5.js sketch that draws the papercut README cover art (1280x427).
// ABOUTME: A fixed seed makes the output the same on each run; press "s" to save a PNG.

const W = 1280;
const H = 427;
const SEED = 7;

const PAPER = "#f2ede3";
const INK = "#1d1b19";
const FAINT = "#d9d2c4";
const LOG = "#c4bcad";
const BLOOD = "#c8322a";
const THREAD = "#2f5d8a";

function setup() {
  createCanvas(W, H);
  pixelDensity(2);
  randomSeed(SEED);
  noiseSeed(SEED);
  noLoop();
}

function draw() {
  background(PAPER);
  drawGrain();
  drawRules();
  const hotspots = [
    { x: 880, y: 120, n: 7 },
    { x: 1070, y: 225, n: 5 },
    { x: 800, y: 310, n: 9 },
    { x: 1150, y: 360, n: 3 },
  ];
  const cuts = [];
  for (const h of hotspots) {
    for (let i = 0; i < h.n; i++) {
      cuts.push({
        x: h.x + randomGaussian(0, 34),
        y: h.y + randomGaussian(0, 15),
        len: random(36, 78),
        ang: radians(-28 + randomGaussian(0, 7)),
        hot: h,
      });
    }
  }
  // Single strays: friction that happened once.
  for (let i = 0; i < 6; i++) {
    cuts.push({
      x: random(680, 1230),
      y: random(50, 390),
      len: random(24, 44),
      ang: radians(-28 + randomGaussian(0, 12)),
      hot: null,
    });
  }
  drawTriageThreads(hotspots);
  for (const c of cuts) drawCut(c);
  for (const h of hotspots.slice(0, 2)) drawStitches(h);
  drawTitle();
}

function drawGrain() {
  noStroke();
  for (let i = 0; i < 9000; i++) {
    const x = random(W);
    const y = random(H);
    fill(0, random(4, 12));
    rect(x, y, 1, 1);
  }
  // Soft vignette toward the edges.
  noFill();
  for (let r = 0; r < 60; r++) {
    stroke(60, 45, 20, map(r, 0, 60, 10, 0));
    strokeWeight(2);
    rect(r, r, W - 2 * r, H - 2 * r);
  }
}

function drawRules() {
  // Ruled lines with log-like dashes: the quiet record of each session.
  for (let y = 46; y < H - 26; y += 24) {
    stroke(FAINT);
    strokeWeight(1);
    line(40, y, W - 40, y);
    let x = 640 + random(-10, 30);
    strokeCap(ROUND);
    stroke(LOG);
    strokeWeight(3);
    while (x < W - 70) {
      const w = random(14, 70);
      if (random() < 0.8) line(x, y - 7, min(x + w, W - 70), y - 7);
      x += w + random(8, 18);
    }
  }
}

function drawCut(c) {
  push();
  translate(c.x, c.y);
  rotate(c.ang);
  const half = c.len / 2;
  // Parted paper edges: a thin lens with a dark gap.
  noStroke();
  fill(BLOOD);
  beginShape();
  for (let t = -half; t <= half; t += 2) {
    const g = 2.6 * sin(map(t, -half, half, 0, PI));
    vertex(t, -g);
  }
  for (let t = half; t >= -half; t -= 2) {
    const g = 2.6 * sin(map(t, -half, half, 0, PI));
    vertex(t, g * 0.6);
  }
  endShape(CLOSE);
  stroke(INK);
  strokeWeight(0.8);
  line(-half, 0, half, 0);
  // A small bead of red at the deepest point.
  noStroke();
  fill(BLOOD);
  const bx = random(-half * 0.3, half * 0.3);
  ellipse(bx, 4, random(3, 6), random(4, 8));
  if (random() < 0.35) {
    fill(red(color(BLOOD)), 50, 42, 200);
    ellipse(bx + 1, 11, 3, 5);
  }
  pop();
}

function drawTriageThreads(hotspots) {
  // Triage groups duplicates: a thread loops through each cluster to one knot.
  noFill();
  stroke(THREAD);
  strokeWeight(1.4);
  drawingContext.setLineDash([2, 6]);
  const knot = { x: 1180, y: 70 };
  for (const h of hotspots) {
    bezier(h.x, h.y, h.x + 40, h.y - 100, knot.x - 120, knot.y + 40, knot.x, knot.y);
  }
  drawingContext.setLineDash([]);
  fill(THREAD);
  noStroke();
  ellipse(knot.x, knot.y, 12, 12);
  fill(PAPER);
  ellipse(knot.x, knot.y, 4, 4);
}

function drawStitches(h) {
  // Closed friction: cross-stitches over the densest cut in the cluster.
  push();
  translate(h.x, h.y);
  rotate(radians(-28));
  stroke(THREAD);
  strokeWeight(2.2);
  strokeCap(ROUND);
  for (let t = -30; t <= 30; t += 12) {
    line(t - 4, -9, t + 4, 9);
  }
  pop();
}

function drawTitle() {
  noStroke();
  fill(INK);
  textFont("Menlo");
  textStyle(BOLD);
  textSize(96);
  textAlign(LEFT, BASELINE);
  text("papercut", 72, 212);

  // One cut through the title itself.
  push();
  translate(446, 178);
  rotate(radians(-28));
  stroke(PAPER);
  strokeWeight(5);
  line(-58, 0, 58, 0);
  stroke(BLOOD);
  strokeWeight(1.6);
  line(-54, 1.5, 54, 1.5);
  pop();

  textStyle(NORMAL);
  textSize(21);
  fill("#5b544b");
  text("coding agents record friction.", 76, 262);
  text("you fix it for good.", 76, 294);
}

function keyPressed() {
  if (key === "s") saveCanvas("cover", "png");
}
