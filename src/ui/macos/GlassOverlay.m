// AppKit samples the playing video. A transparent sibling carries egui's
// foreground at the display's pixel density without creating another window.
#define GL_SILENCE_DEPRECATION
#import <AppKit/AppKit.h>
#import <OpenGL/gl3.h>
#import <QuartzCore/QuartzCore.h>

@protocol MTGlassMaterial <NSObject>
@property CGFloat cornerRadius;
@property NSInteger style;
@property (copy) NSColor *tintColor;
@end

@interface MTGlassOverlay : NSView {
@public
  GLuint framebuffer, colorBuffer;
  GLint previousDraw, previousRead, previousRender;
  GLsizei width, height;
}
@property NSView<MTGlassMaterial> *material;
@property NSView *foreground;
@property NSImageView *symbol;
@property const char *glyph;
@end

@implementation MTGlassOverlay
- (NSView *)hitTest:(NSPoint)point { return nil; }
@end

NSView *mytube_glass_create(void) NS_RETURNS_RETAINED;
NSView *mytube_glass_create(void) {
  MTGlassOverlay *host = [MTGlassOverlay new];
  host.material = [NSClassFromString(@"NSGlassEffectView") new];
  host.foreground = [NSView new];
  host.foreground.wantsLayer = YES;
  for (NSView *v in @[host.material, host.foreground]) {
    v.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    [host addSubview:v];
  }
  host.accessibilityElement = NO;
  return host;
}

void mytube_glass_style(MTGlassOverlay *host, CGFloat radius, CGFloat edgeRadius, uint8_t edges, CGFloat opacity, const char *glyph) {
  if (edges && !host.wantsLayer) host.wantsLayer = YES;
  CALayer *layer = host.layer;
  if (layer.cornerRadius != edgeRadius) layer.cornerRadius = edgeRadius;
  if (edges && layer.maskedCorners != edges) layer.maskedCorners = edges;
  if (layer.masksToBounds != (edges != 0)) layer.masksToBounds = edges != 0;
  if (host.glyph != glyph) {
    host.glyph = glyph;
    host.material.style = glyph ? 1 : 0; // clear media glass or regular panel glass
    host.material.tintColor = glyph ? [NSColor.blackColor colorWithAlphaComponent:0.22] : nil;
    if (glyph) {
      if (!host.symbol) {
        host.symbol = [NSImageView new];
        host.symbol.imageScaling = NSImageScaleNone;
        host.symbol.contentTintColor = NSColor.whiteColor;
        host.symbol.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        [host addSubview:host.symbol];
      }
      NSString *name = [NSString stringWithUTF8String:glyph];
      NSImageSymbolConfiguration *config = [NSImageSymbolConfiguration configurationWithPointSize:
        [name hasSuffix:@".10"] ? 23 : 28 weight:NSFontWeightSemibold];
      host.symbol.image = [[NSImage imageWithSystemSymbolName:name accessibilityDescription:nil] imageWithSymbolConfiguration:config];
      host.symbol.frame = host.bounds;
    }
    host.symbol.hidden = !glyph;
  }
  if (host.symbol && host.symbol.alphaValue != opacity) host.symbol.alphaValue = opacity;
  if (host.material.cornerRadius != radius) host.material.cornerRadius = radius;
  if (host.material.alphaValue != opacity) host.material.alphaValue = opacity;
}

void mytube_glass_begin(MTGlassOverlay *host, int width, int height) {
  host->width = width;
  host->height = height;
  glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &host->previousDraw);
  glGetIntegerv(GL_READ_FRAMEBUFFER_BINDING, &host->previousRead);
  glGetIntegerv(GL_RENDERBUFFER_BINDING, &host->previousRender);
  glGenFramebuffers(1, &host->framebuffer);
  glBindFramebuffer(GL_FRAMEBUFFER, host->framebuffer);
  glGenRenderbuffers(1, &host->colorBuffer);
  glBindRenderbuffer(GL_RENDERBUFFER, host->colorBuffer);
  glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA8, width, height);
  glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, host->colorBuffer);
  glDisable(GL_SCISSOR_TEST);
  glClearColor(0, 0, 0, 0);
  glClear(GL_COLOR_BUFFER_BIT);
}

void mytube_glass_end(MTGlassOverlay *host) {
  GLsizei w = host->width, h = host->height;
  NSMutableData *rgba = [NSMutableData dataWithLength:(NSUInteger)w * h * 4];
  glReadPixels(0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, rgba.mutableBytes);
  glBindFramebuffer(GL_DRAW_FRAMEBUFFER, host->previousDraw);
  glBindFramebuffer(GL_READ_FRAMEBUFFER, host->previousRead);
  glBindRenderbuffer(GL_RENDERBUFFER, host->previousRender);
  glDeleteFramebuffers(1, &host->framebuffer);
  glDeleteRenderbuffers(1, &host->colorBuffer);
  // GL starts at the bottom-left; Core Graphics images start with the top row.
  size_t stride = (size_t)w * 4;
  NSMutableData *scratch = [NSMutableData dataWithLength:stride];
  uint8_t *pixels = rgba.mutableBytes;
  for (GLsizei y = 0; y < h / 2; ++y) {
    uint8_t *top = pixels + y * stride;
    uint8_t *bottom = pixels + (h - y - 1) * stride;
    memcpy(scratch.mutableBytes, top, stride);
    memcpy(top, bottom, stride);
    memcpy(bottom, scratch.bytes, stride);
  }
  CGDataProviderRef data = CGDataProviderCreateWithCFData((__bridge CFDataRef)rgba);
  CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  CGImageRef image = CGImageCreate(w, h, 8, 32, w * 4, space, (CGBitmapInfo)kCGImageAlphaPremultipliedLast,
                                    data, NULL, false, kCGRenderingIntentDefault);
  [CATransaction begin];
  [CATransaction setDisableActions:YES];
  host.foreground.layer.contents = (__bridge id)image;
  [CATransaction commit];
  CGImageRelease(image);
  CGColorSpaceRelease(space);
  CGDataProviderRelease(data);
}

// Egui owns layout, accessibility and hit testing. AppKit draws the actual
// glass bezel, SF Symbol and title in the same rectangle.
@interface MTCatalogButton : NSButton
@property NSTextField *caption;
@property NSImageView *indicator;
@end
@implementation MTCatalogButton
- (NSView *)hitTest:(NSPoint)point { return nil; }
- (BOOL)isAccessibilityElement { return NO; }
- (void)layout {
  [super layout];
  NSRect bounds = self.bounds;
  CGFloat textHeight = self.caption.fittingSize.height;
  if (self.indicator.hidden) { self.caption.frame = NSMakeRect(4, floor(NSMidY(bounds) - textHeight / 2), MAX(1, NSWidth(bounds) - 8), textHeight); return; }
  self.caption.frame = NSMakeRect(9, floor(NSMidY(bounds) - textHeight / 2), MAX(1, NSWidth(bounds) - 29), textHeight);
  self.indicator.frame = NSMakeRect(NSMaxX(bounds) - 21, NSMidY(bounds) - 6, 12, 12);
}
@end

NSView *mytube_catalog_button_create(void) NS_RETURNS_RETAINED;
NSView *mytube_catalog_button_create(void) {
  MTCatalogButton *button = [MTCatalogButton buttonWithTitle:@"" target:nil action:nil];
  if (@available(macOS 26, *)) {
    button.bezelStyle = (NSBezelStyle)16; // glass
    [button setValue:@1 forKey:@"borderShape"]; // capsule
  }
  button.caption = [NSTextField labelWithString:@""];
  button.caption.alignment = NSTextAlignmentCenter;
  button.caption.lineBreakMode = NSLineBreakByTruncatingTail;
  button.caption.font = [NSFont systemFontOfSize:13 weight:NSFontWeightMedium];
  button.indicator = [NSImageView new];
  [button addSubview:button.caption];
  [button addSubview:button.indicator];
  return button;
}
void mytube_catalog_button_update(MTCatalogButton *button, NSString *title, NSString *symbol, bool selected, bool hot, bool dark, double strength) {
  button.appearance = [NSAppearance appearanceNamed:dark ? NSAppearanceNameDarkAqua : NSAppearanceNameAqua];
  button.caption.stringValue = title;
  NSColor *ink = (dark || selected) ? NSColor.whiteColor : NSColor.blackColor;
  button.caption.textColor = ink;
  button.indicator.hidden = !symbol.length;
  button.indicator.image = symbol.length ? [NSImage imageWithSystemSymbolName:symbol accessibilityDescription:nil] : nil;
  button.indicator.contentTintColor = ink;
  button.bezelColor = selected ? NSColor.controlAccentColor :
      (strength < 1 ? [NSColor.windowBackgroundColor colorWithAlphaComponent:1 - strength] : nil);
  [button highlight:hot];
  button.needsLayout = YES;
}

@interface MTMediaButton : NSButton
@end
@implementation MTMediaButton
- (NSView *)hitTest:(NSPoint)point { return nil; }
- (BOOL)isAccessibilityElement { return NO; }
@end

NSView *mytube_media_button(void) NS_RETURNS_RETAINED;
NSView *mytube_media_button(void) {
  MTMediaButton *button = [MTMediaButton buttonWithTitle:@"" target:nil action:nil];
  button.bezelStyle = (NSBezelStyle)16; // NSBezelStyleGlass
  [button setValue:@1 forKey:@"borderShape"]; // capsule
  button.imagePosition = NSImageOnly;
  button.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
  button.contentTintColor = NSColor.whiteColor;
  return button;
}
void mytube_media_style(NSView *view, NSString *name, CGFloat height, CGFloat strength) {
  NSButton *button = (NSButton *)view;
  NSImageSymbolConfiguration *config = [NSImageSymbolConfiguration configurationWithPointSize:MAX(8, height * 0.42) weight:NSFontWeightSemibold];
  button.image = [[NSImage imageWithSystemSymbolName:name accessibilityDescription:nil] imageWithSymbolConfiguration:config];
  button.bezelColor = [NSColor.blackColor colorWithAlphaComponent:1.0 - 0.8 * strength];
}

// Rasterize the system's borderless PiP glyph once for both egui viewports.
bool mytube_media_glyph(const char *name, size_t side, unsigned char *alpha) {
  NSImage *symbol = [NSImage imageWithSystemSymbolName:[NSString stringWithUTF8String:name] accessibilityDescription:nil];
  symbol = [symbol imageWithSymbolConfiguration:[NSImageSymbolConfiguration
    configurationWithPointSize:side weight:NSFontWeightSemibold]];
  CGImageRef glyph = [symbol CGImageForProposedRect:NULL context:nil hints:nil];
  if (!glyph) return false;
  CGContextRef bitmap = CGBitmapContextCreate(alpha, side, side, 8, side, NULL, (CGBitmapInfo)kCGImageAlphaOnly);
  if (!bitmap) return false;
  CGFloat width = CGImageGetWidth(glyph), height = CGImageGetHeight(glyph);
  CGFloat scale = side / MAX(width, height);
  CGRect bounds = CGRectMake((side - width * scale) / 2, (side - height * scale) / 2, width * scale, height * scale);
  CGContextDrawImage(bitmap, bounds, glyph);
  CGContextRelease(bitmap);
  return true;
}

// Egui draws its popups and dialogs into the surface underneath the native
// buttons. Cut out only the covered part of each bezel while a popup is visible.
void mytube_catalog_button_clip(NSView *button, const NSRect *cuts, size_t count) {
  if (!count) {
    button.layer.mask = nil;
    return;
  }
  button.wantsLayer = YES;
  CGPathRef bounds = CGPathCreateWithRect(button.bounds, NULL);
  CGMutablePathRef cover = CGPathCreateMutable();
  CGPathAddRects(cover, NULL, cuts, count);
  CGPathRef visible = CGPathCreateCopyBySubtractingPath(bounds, cover, false);
  CAShapeLayer *mask = [CAShapeLayer layer];
  mask.frame = button.bounds;
  mask.path = visible;
  button.layer.mask = mask;
  CGPathRelease(visible);
  CGPathRelease(cover);
  CGPathRelease(bounds);
}
