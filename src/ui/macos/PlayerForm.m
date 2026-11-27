// AppKit owns editing, keyboard navigation and accessibility for the settings
// viewport.
#import <AppKit/AppKit.h>
#import <QuartzCore/CALayer.h>

@interface MTDocument : NSView
@end
@implementation MTDocument
- (BOOL)isFlipped {
  return YES;
}
@end

@interface MTRateSlider : NSSlider
@property double step;
- (void)snapToStep;
@end
@implementation MTRateSlider
- (void)snapToStep {
  if (_step > 0)
    self.doubleValue = self.minValue + round((self.doubleValue - self.minValue) / _step) * _step;
}
- (BOOL)adjustBySteps:(NSInteger)count {
  if (!self.enabled) return NO;
  self.doubleValue += count * self.step;
  [self snapToStep];
  return [self sendAction:self.action to:self.target];
}
- (void)keyDown:(NSEvent *)event {
  NSString *key = event.charactersIgnoringModifiers;
  if (key.length == 1) {
    switch ([key characterAtIndex:0]) {
      case NSLeftArrowFunctionKey:
      case NSDownArrowFunctionKey: [self adjustBySteps:-1]; return;
      case NSRightArrowFunctionKey:
      case NSUpArrowFunctionKey: [self adjustBySteps:1]; return;
    }
  }
  [super keyDown:event];
}
- (BOOL)accessibilityPerformIncrement { return [self adjustBySteps:1]; }
- (BOOL)accessibilityPerformDecrement { return [self adjustBySteps:-1]; }
@end

@protocol MTRateReceiver <NSObject>
- (void)setPlaybackRate:(double)rate;
@end

@interface MTMenuRateView : MTDocument
@property (strong) id<MTRateReceiver> receiver;
@property NSTextField *readout;
@end
@implementation MTMenuRateView
- (void)adjust:(MTRateSlider *)slider {
  [slider snapToStep];
  self.readout.stringValue = [NSString stringWithFormat:@"%.2f×", slider.doubleValue];
  [self.receiver setPlaybackRate:slider.doubleValue];
}
@end

NSView *mytube_menu_rate_view(double rate, double minimum, double maximum,
                              double step, id<MTRateReceiver> receiver) NS_RETURNS_RETAINED;
NSView *mytube_menu_rate_view(double rate, double minimum, double maximum,
                              double step, id<MTRateReceiver> receiver) {
  MTMenuRateView *view = [[MTMenuRateView alloc] initWithFrame:NSMakeRect(0, 0, 224, 52)];
  view.receiver = receiver;
  NSTextField *label = [NSTextField labelWithString:@"Playback speed"];
  label.frame = NSMakeRect(14, 4, 132, 19);
  [view addSubview:label];
  view.readout = [NSTextField labelWithString:[NSString stringWithFormat:@"%.2f×", rate]];
  view.readout.frame = NSMakeRect(144, 4, 66, 19);
  view.readout.alignment = NSTextAlignmentRight;
  view.readout.textColor = NSColor.secondaryLabelColor;
  [view addSubview:view.readout];
  MTRateSlider *slider = [MTRateSlider new];
  slider.frame = NSMakeRect(14, 24, 196, 22);
  slider.minValue = minimum;
  slider.maxValue = maximum;
  slider.step = step;
  slider.doubleValue = rate;
  slider.continuous = YES;
  slider.target = view;
  slider.action = @selector(adjust:);
  slider.accessibilityLabel = @"Playback speed";
  [view addSubview:slider];
  return view;
}

@interface MTFormButton : NSButton
@property NSTrackingArea *hoverArea;
@end

@implementation MTFormButton
- (void)updateTrackingAreas {
  [super updateTrackingAreas];
  if (_hoverArea) [self removeTrackingArea:_hoverArea];
  _hoverArea = [[NSTrackingArea alloc] initWithRect:NSZeroRect
    options:NSTrackingMouseEnteredAndExited | NSTrackingActiveInKeyWindow | NSTrackingInVisibleRect
    owner:self userInfo:nil];
  [self addTrackingArea:_hoverArea];
}
- (void)mouseEntered:(NSEvent *)event { self.needsDisplay = YES; }
- (void)mouseExited:(NSEvent *)event { self.needsDisplay = YES; }
- (void)drawRect:(NSRect)dirty {
  [super drawRect:dirty];
  if (self.enabled && self.window.isKeyWindow &&
      NSPointInRect([self convertPoint:self.window.mouseLocationOutsideOfEventStream fromView:nil], self.visibleRect)) {
    [[NSColor.labelColor colorWithAlphaComponent:0.07] setFill];
    [[NSBezierPath bezierPathWithRoundedRect:NSInsetRect(self.bounds, 1, 1) xRadius:6 yRadius:6] fill];
  }
}
@end

@interface MTDisclosureCell : NSButtonCell
@property BOOL expanded;
@end

@implementation MTDisclosureCell
- (void)drawInteriorWithFrame:(NSRect)frame inView:(NSView *)view {
  if (self.isHighlighted) {
    [[NSColor.labelColor colorWithAlphaComponent:0.09] setFill];
    [[NSBezierPath bezierPathWithRoundedRect:frame xRadius:8 yRadius:8] fill];
  }

  NSRect title = NSInsetRect(frame, 10, 0);
  title.size.width = MAX(1, NSWidth(title) - 20);
  [super drawInteriorWithFrame:title inView:view];

  CGFloat x = NSMaxX(frame) - 15, y = NSMidY(frame);
  CGFloat down = view.isFlipped ? 1 : -1;
  NSBezierPath *arrow = [NSBezierPath bezierPath];
  [arrow moveToPoint:_expanded ? NSMakePoint(x - 4, y - 2 * down) : NSMakePoint(x - 2, y - 4 * down)];
  [arrow lineToPoint:_expanded ? NSMakePoint(x, y + 2 * down) : NSMakePoint(x + 2, y)];
  [arrow lineToPoint:_expanded ? NSMakePoint(x + 4, y - 2 * down) : NSMakePoint(x - 2, y + 4 * down)];
  arrow.lineWidth = 1.4;
  arrow.lineCapStyle = NSLineCapStyleRound;
  arrow.lineJoinStyle = NSLineJoinStyleRound;
  [NSColor.secondaryLabelColor setStroke];
  [arrow stroke];

  [NSColor.separatorColor setFill];
  NSRectFillUsingOperation(NSMakeRect(NSMinX(frame) + 10, view.isFlipped ? NSMaxY(frame) - 1 : NSMinY(frame),
                                      MAX(0, NSWidth(frame) - 20), 1), NSCompositingOperationSourceOver);
}
@end

@interface MTSpeedOptions : NSControl
@property CGFloat buttonWidth;
@property NSInteger selection;
- (CGFloat)heightForWidth:(CGFloat)width;
@end
@implementation MTSpeedOptions
- (BOOL)isFlipped { return YES; }
- (instancetype)initWithTitles:(NSArray<NSString *> *)titles {
  if (!(self = [super initWithFrame:NSZeroRect])) return nil;
  self.accessibilityElement = YES;
  self.accessibilityRole = NSAccessibilityGroupRole;
  self.accessibilityLabel = @"Playback speed presets";
  for (NSString *title in titles) {
    MTFormButton *button = [MTFormButton buttonWithTitle:title target:self action:@selector(choose:)];
    [button setButtonType:NSButtonTypePushOnPushOff];
    button.bezelStyle = NSBezelStyleAccessoryBarAction;
    button.tag = self.subviews.count;
    button.accessibilityLabel = [title isEqual:@"Normal"] ? @"Normal playback speed" : title;
    [self addSubview:button];
    _buttonWidth = MAX(_buttonWidth, ceil(button.fittingSize.width));
  }
  self.accessibilityChildren = NSAccessibilityUnignoredChildren(self.subviews);
  return self;
}
- (void)setEnabled:(BOOL)enabled {
  [super setEnabled:enabled];
  for (NSButton *button in self.subviews) button.enabled = enabled;
}
- (void)choose:(NSButton *)sender {
  self.integerValue = sender.tag;
  [self sendAction:self.action to:self.target];
}
- (void)setIntegerValue:(NSInteger)value {
  _selection = value;
  for (NSButton *button in self.subviews)
    button.state = button.tag == value ? NSControlStateValueOn : NSControlStateValueOff;
}
- (NSInteger)integerValue { return _selection; }
- (NSInteger)columnsForWidth:(CGFloat)width {
  return MAX(1, MIN((NSInteger)self.subviews.count, (NSInteger)((width + 4) / (_buttonWidth + 4))));
}
- (CGFloat)heightForWidth:(CGFloat)width {
  NSInteger columns = [self columnsForWidth:width];
  return ceil(self.subviews.count / (CGFloat)columns) * 32 - 4;
}
- (void)layout {
  [super layout];
  NSInteger columns = [self columnsForWidth:NSWidth(self.bounds)];
  CGFloat width = (NSWidth(self.bounds) - (columns - 1) * 4) / columns;
  for (NSButton *button in self.subviews) {
    NSInteger i = button.tag;
    button.frame = NSMakeRect((i % columns) * (width + 4), (i / columns) * 32, width, 28);
  }
}
@end

@interface MTPlayerForm : MTDocument
@property NSScrollView *scroll;
@property NSTextField *heading;
@property MTDocument *document;
@property NSSegmentedControl *tabs;
@property NSArray<NSSegmentedControl *> *moreTabs;
@property NSButton *closeButton;
@property NSMutableArray *events;
@property NSMutableDictionary<NSString *, NSControl *> *controls;
@property NSArray *rows;
@property (copy) NSArray *previousWindowChildren;
@property NSString *section;
@property BOOL compact;
@property BOOL sidebar;
@property CGFloat labeledTabWidth;
@property NSArray<NSString *> *tabKeys;
@property NSArray<NSString *> *moreTabKeys;
@end

@implementation MTPlayerForm
- (instancetype)initWithFrame:(NSRect)frame {
  if (!(self = [super initWithFrame:frame]))
    return nil;
  _heading = [NSTextField labelWithString:@"Settings"];
  _heading.font = [NSFont systemFontOfSize:18 weight:NSFontWeightSemibold];
  _events = [NSMutableArray new];
  _controls = [NSMutableDictionary new];
  _scroll = [NSScrollView new];
  _scroll.drawsBackground = NO;
  _scroll.hasVerticalScroller = YES;
  _scroll.scrollerStyle = NSScrollerStyleOverlay;
  _scroll.verticalScroller.controlSize = NSControlSizeSmall;
  _scroll.autohidesScrollers = YES;
  _document = [MTDocument new];
  _scroll.documentView = _document;

  _tabs = [NSSegmentedControl
      segmentedControlWithLabels:@[]
                    trackingMode:NSSegmentSwitchTrackingSelectOne
                          target:self
                          action:@selector(selectTab:)];
  _tabs.segmentDistribution = NSSegmentDistributionFillEqually;
  NSMutableArray<NSSegmentedControl *> *moreTabs = [NSMutableArray new];
  for (NSInteger i = 0; i < 2; i++) {
    NSSegmentedControl *strip = [NSSegmentedControl
        segmentedControlWithLabels:@[]
                      trackingMode:NSSegmentSwitchTrackingSelectOne
                            target:self
                            action:@selector(selectTab:)];
    strip.segmentDistribution = NSSegmentDistributionFillEqually;
    strip.controlSize = NSControlSizeSmall;
    strip.accessibilityLabel = @"Settings categories";
    strip.hidden = YES;
    [moreTabs addObject:strip];
  }
  _moreTabs = moreTabs;
  _closeButton = [NSButton
      buttonWithImage:[NSImage imageWithSystemSymbolName:@"xmark"
                                accessibilityDescription:@"Close settings"]
               target:self
               action:@selector(close:)];
  _closeButton.bordered = NO;
  for (NSView *view in @[ _heading, _closeButton, _tabs ])
    [self addSubview:view];
  for (NSView *view in _moreTabs) [self addSubview:view];
  [self addSubview:_scroll];
  self.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
  return self;
}
- (void)selectTab:(NSSegmentedControl *)sender {
  NSInteger selected = sender.selectedSegment;
  if (selected < 0) return;
  NSUInteger index = sender == _tabs ? selected : sender.tag + selected;
  NSArray<NSString *> *keys = sender == _tabs ? _tabKeys : _moreTabKeys;
  if (index < keys.count) [_events addObject:@[ @"section", keys[index] ]];
}
- (void)close:(id)sender {
  [_events addObject:@[ @"close", @YES ]];
}
- (void)setFrameSize:(NSSize)size {
  if (NSEqualSizes(self.frame.size, size)) return;
  [super setFrameSize:size];
  self.needsLayout = YES;
}
- (void)changed:(NSControl *)sender {
  id value = @YES;
  if ([sender isKindOfClass:NSTextField.class])
    value = sender.stringValue;
  if ([sender isKindOfClass:NSSegmentedControl.class])
    value = @([(NSSegmentedControl *)sender selectedSegment]);
  if ([sender isKindOfClass:NSSlider.class]) {
    if ([sender isKindOfClass:MTRateSlider.class]) [(MTRateSlider *)sender snapToStep];
    value = @(sender.doubleValue);
  }
  if ([sender isKindOfClass:MTSpeedOptions.class])
    value = @(sender.integerValue);
  if ([sender isKindOfClass:NSPopUpButton.class])
    value = @([(NSPopUpButton *)sender indexOfSelectedItem]);
  if ([sender isKindOfClass:NSColorWell.class]) {
    NSColor *color = [[(NSColorWell *)sender color]
        colorUsingColorSpace:NSColorSpace.sRGBColorSpace];
    value = @[
      @(lround(color.redComponent * 255)),
      @(lround(color.greenComponent * 255)),
      @(lround(color.blueComponent * 255)),
      @(lround(color.alphaComponent * 255))
    ];
  }
  if ([sender isKindOfClass:NSSlider.class] &&
      [[_events.lastObject firstObject] isEqual:sender.identifier])
    [_events removeLastObject];
  [_events addObject:@[ sender.identifier, value ]];
}
- (NSControl *)makeControl:(NSString *)kind label:(NSString *)label {
  NSControl *control;
  if ([kind isEqual:@"label"] || [kind isEqual:@"heading"]) {
    NSTextField *text = [NSTextField wrappingLabelWithString:label];
    if ([kind isEqual:@"heading"]) {
      text.font = [NSFont systemFontOfSize:13 weight:NSFontWeightSemibold];
      text.accessibilityRole = NSAccessibilityHeadingRole;
    } else {
      text.selectable = YES;
    }
    return text;
  } else if ([kind isEqual:@"segments"])
    control = [NSSegmentedControl new];
  else if ([kind isEqual:@"choice"])
    control = [NSPopUpButton new];
  else if ([kind isEqual:@"number"])
    control = [NSSlider new];
  else if ([kind isEqual:@"fineSpeed"])
    control = [MTRateSlider new];
  else if ([kind isEqual:@"text"])
    control = [NSTextField new];
  else if ([kind isEqual:@"password"])
    control = [NSSecureTextField new];
  else if ([kind isEqual:@"color"])
    control = [NSColorWell new];
  else if ([kind isEqual:@"toggle"])
    control = [NSButton checkboxWithTitle:label
                                   target:self
                                   action:@selector(changed:)];
  else if ([kind isEqual:@"radio"])
    control = [NSButton radioButtonWithTitle:label
                                      target:self
                                      action:@selector(changed:)];
  else if ([kind isEqual:@"disclosure"]) {
    MTFormButton *button = [MTFormButton new];
    button.cell = [[MTDisclosureCell alloc] initTextCell:label];
    button.bordered = NO;
    button.alignment = NSTextAlignmentLeft;
    button.font = [NSFont systemFontOfSize:13 weight:NSFontWeightMedium];
    button.accessibilityRole = NSAccessibilityDisclosureTriangleRole;
    control = button;
  }
  else
    control = [NSButton buttonWithTitle:label
                                 target:self
                                 action:@selector(changed:)];
  if ([kind isEqual:@"toggle"] || [kind isEqual:@"radio"] || [kind isEqual:@"disclosure"]) {
    control.cell.wraps = YES;
    control.cell.lineBreakMode = NSLineBreakByWordWrapping;
  }
  control.target = self;
  control.action = @selector(changed:);
  [control setAccessibilityLabel:label];
  control.toolTip = label;
  return control;
}
- (void)updateForm:(NSDictionary *)model {
  NSString *section = model[@"section"];
  BOOL sidebar = [model[@"sidebar"] boolValue];
  BOOL compact = [model[@"compact"] boolValue];
  if (!_moreTabKeys) {
    NSArray *tabs = model[@"tabs"];
    NSUInteger primaryCount = MIN(4, tabs.count);
    _tabKeys = [tabs subarrayWithRange:NSMakeRange(0, primaryCount)];
    _tabs.segmentCount = _tabKeys.count;
    _moreTabKeys = [tabs subarrayWithRange:NSMakeRange(primaryCount, tabs.count - primaryCount)];
    NSUInteger offset = 0;
    NSUInteger stripsLeft = _moreTabs.count;
    for (NSSegmentedControl *strip in _moreTabs) {
      strip.tag = offset;
      strip.segmentCount = (_moreTabKeys.count - offset + stripsLeft - 1) / stripsLeft;
      stripsLeft--;
      for (NSInteger index = 0; index < strip.segmentCount; index++) {
        [strip setLabel:_moreTabKeys[offset++] forSegment:index];
      }
    }
  }
  if (![_section isEqual:section] || sidebar != _sidebar || compact != _compact) {
    _sidebar = sidebar;
    _compact = compact;
    if (![_section isEqual:section]) [_scroll.contentView scrollToPoint:NSZeroPoint];
    _section = section;
    NSArray *symbols = @[@"paintbrush.fill", @"play.fill", @"photo", @"waveform"];
    NSInteger selected = [_tabKeys indexOfObject:section];
    NSUInteger selectedMore = [_moreTabKeys indexOfObject:section];
    BOOL showMore = selectedMore != NSNotFound;
    _tabs.selectedSegment = selected == NSNotFound ? -1 : selected;
    for (NSSegmentedControl *strip in _moreTabs) {
      strip.hidden = NO;
      strip.selectedSegment = showMore && NSLocationInRange(selectedMore, NSMakeRange(strip.tag, strip.segmentCount))
          ? (NSInteger)selectedMore - strip.tag : -1;
    }
    _tabs.controlSize = sidebar ? NSControlSizeLarge : NSControlSizeRegular;
    _tabs.segmentDistribution = sidebar ? NSSegmentDistributionFit : NSSegmentDistributionFillEqually;
    for (NSInteger i = 0; i < _tabs.segmentCount; i++) {
      NSString *name = _tabKeys[i];
      NSImage *image = sidebar ? [NSImage imageWithSystemSymbolName:symbols[i] accessibilityDescription:name] : nil;
      [_tabs setImage:image forSegment:i];
      [_tabs setToolTip:name forSegment:i];
      [_tabs setLabel:(!sidebar || i == _tabs.selectedSegment) ? name : @"" forSegment:i];
    }
    _labeledTabWidth = _tabs.fittingSize.width;
    _heading.hidden = sidebar;
    _closeButton.bordered = sidebar;
    _closeButton.bezelStyle = NSBezelStyleCircular;
  }
  NSArray *rows = model[@"rows"];
  NSMutableSet *used = [NSMutableSet new];
  for (NSDictionary *row in rows) {
    NSString *key = row[@"id"], *kind = row[@"kind"], *label = row[@"label"];
    id value = row[@"value"];
    NSControl *c = _controls[key];
    if (!c) {
      if ([kind isEqual:@"speed"]) {
        c = [[MTSpeedOptions alloc] initWithTitles:row[@"options"]];
        c.target = self;
        c.action = @selector(changed:);
      } else {
        c = [self makeControl:kind label:label];
      }
      c.identifier = key;
      _controls[key] = c;
      [_document addSubview:c];
    }
    [used addObject:key];
    c.enabled = [row[@"enabled"] boolValue];
    if ([kind isEqual:@"speed"]) {
      c.integerValue = [value isKindOfClass:NSNumber.class] ? [value integerValue] : -1;
    } else if ([kind isEqual:@"segments"]) {
      NSSegmentedControl *segments = (id)c;
      NSArray *titles = row[@"options"];
      segments.segmentCount = titles.count;
      segments.trackingMode = NSSegmentSwitchTrackingSelectOne;
      for (NSUInteger i = 0; i < titles.count; i++)
        [segments setLabel:titles[i] forSegment:i];
      segments.selectedSegment = [value integerValue];
      NSString *menuKey = [key stringByAppendingString:@"-menu"];
      NSPopUpButton *menu = (id)_controls[menuKey];
      if (!menu) {
        menu = (id)[self makeControl:@"choice" label:label];
        menu.identifier = key;
        _controls[menuKey] = menu;
        [_document addSubview:menu];
      }
      if (![menu.itemTitles isEqual:titles]) {
        [menu removeAllItems];
        [menu addItemsWithTitles:titles];
      }
      [menu selectItemAtIndex:segments.selectedSegment];
      menu.enabled = c.enabled;
      [used addObject:menuKey];
    } else if ([kind isEqual:@"choice"]) {
      NSPopUpButton *popup = (id)c;
      if (![popup.itemTitles isEqual:row[@"options"]]) {
        [popup removeAllItems];
        [popup addItemsWithTitles:row[@"options"]];
      }
      [popup selectItemWithTitle:value];
    } else if ([kind isEqual:@"toggle"] || [kind isEqual:@"radio"])
      ((NSButton *)c).state = [value boolValue];
    else if ([kind isEqual:@"disclosure"]) {
      NSButton *button = (NSButton *)c;
      BOOL expanded = [value boolValue];
      if (button.isAccessibilityExpanded != expanded) {
        ((MTDisclosureCell *)button.cell).expanded = expanded;
        button.accessibilityExpanded = expanded;
        button.needsDisplay = YES;
      }
    } else if ([kind isEqual:@"number"] || [kind isEqual:@"fineSpeed"]) {
      NSSlider *slider = (id)c;
      slider.minValue = [row[@"options"][0] doubleValue];
      slider.maxValue = [row[@"options"][1] doubleValue];
      if ([kind isEqual:@"fineSpeed"]) {
        ((MTRateSlider *)slider).step = [row[@"options"][2] doubleValue];
        slider.accessibilityLabel = @"Playback speed";
      }
      slider.continuous = YES;
      if (!slider.isHighlighted) slider.doubleValue = [value doubleValue];
    } else if ([kind isEqual:@"text"] || [kind isEqual:@"password"]) {
      NSTextField *field = (id)c;
      field.placeholderString = label;
      ((NSTextFieldCell *)field.cell).sendsActionOnEndEditing = YES;
      if (!field.currentEditor && ![field.stringValue isEqual:value])
        field.stringValue = value;
    } else if ([kind isEqual:@"color"]) {
      ((NSColorWell *)c).color =
          [NSColor colorWithSRGBRed:[value[0] doubleValue] / 255
                              green:[value[1] doubleValue] / 255
                               blue:[value[2] doubleValue] / 255
                              alpha:[value[3] doubleValue] / 255];
    }
    if (([kind isEqual:@"segments"] || [kind isEqual:@"speed"] || [kind isEqual:@"number"] || [kind isEqual:@"fineSpeed"] ||
         [kind isEqual:@"choice"] || [kind isEqual:@"color"]) &&
        label.length) {
      NSString *caption = [key stringByAppendingString:@"-label"];
      NSTextField *field = (id)_controls[caption];
      if (!field) {
        field = [NSTextField wrappingLabelWithString:label];
        field.font = [NSFont systemFontOfSize:12 weight:NSFontWeightSemibold];
        field.textColor = [NSColor secondaryLabelColor];
        _controls[caption] = field;
        [_document addSubview:field];
      }
      if ([kind isEqual:@"fineSpeed"])
        field.stringValue = [NSString stringWithFormat:@"%@   %.2f×", label, [value doubleValue]];
      else field.stringValue = [kind isEqual:@"number"]
                              ? [NSString stringWithFormat:@"%@   %.3g", label,
                                                           [value doubleValue]]
                              : label;
      [used addObject:caption];
    }
  }
  for (NSString *key in _controls.allKeys)
    if (![used containsObject:key]) {
      [_controls[key] removeFromSuperview];
      [_controls removeObjectForKey:key];
    }
  _rows = rows;
  self.needsLayout = YES;
}
- (void)layout {
  [super layout];
  CGFloat width = self.bounds.size.width, pad = _compact ? 8 : 14;
  if (_sidebar) {
    CGFloat side = _compact ? 28 : 36;
    _closeButton.frame = NSMakeRect(10, 8, side, side);
    _tabs.frame = NSMakeRect(side + 18, 7, MAX(1, width - side - 28), side + 2);
    NSInteger selected = _tabs.selectedSegment;
    if (selected >= 0) {
      NSString *name = NSWidth(_tabs.frame) >= _labeledTabWidth
                           ? [_tabs toolTipForSegment:selected] : @"";
      if (![[_tabs labelForSegment:selected] isEqual:name])
        [_tabs setLabel:name forSegment:selected];
    }
  } else {
    _closeButton.frame = NSMakeRect(width - pad - 22, pad, 22, 24);
    _heading.frame =
        NSMakeRect(pad, pad, MAX(1, NSMinX(_closeButton.frame) - 2 * pad), 25);
    _tabs.frame =
        NSMakeRect(pad, NSMaxY(_heading.frame) + 8, MAX(0, width - 2 * pad), 28);
  }
  CGFloat top = NSMaxY(_tabs.frame) + 6;
  if (!_moreTabs.firstObject.hidden) {
    CGFloat available = MAX(1, width - 2 * pad);
    CGFloat gap = 6;
    CGFloat fitting = 2 * MAX(_moreTabs.firstObject.intrinsicContentSize.width, _moreTabs.lastObject.intrinsicContentSize.width) + gap;
    BOOL singleRow = available >= fitting;
    CGFloat stripWidth = singleRow ? (available - gap) / 2 : available;
    for (NSUInteger i = 0; i < _moreTabs.count; i++) {
      CGFloat x = singleRow ? pad + i * (stripWidth + gap) : pad;
      CGFloat y = singleRow ? top : top + i * (24 + gap);
      _moreTabs[i].frame = NSMakeRect(x, y, stripWidth, 24);
    }
    top = NSMaxY(_moreTabs.lastObject.frame) + 4;
  }
  _scroll.frame =
      NSMakeRect(0, top, width, MAX(0, self.bounds.size.height - top));
  CGFloat y = pad;
  for (NSDictionary *row in _rows) {
    NSString *key = row[@"id"], *kind = row[@"kind"];
    CGFloat inset = pad + [row[@"depth"] unsignedIntegerValue] * 12;
    CGFloat inner = MAX(1, _scroll.contentSize.width - pad - inset);
    BOOL disclosure = [kind isEqual:@"disclosure"];
    BOOL heading = [kind isEqual:@"heading"];
    if (heading && y > pad) y += 8;
    NSControl *caption = _controls[[key stringByAppendingString:@"-label"]];
    if (caption) {
      NSSize size = [caption.cell cellSizeForBounds:NSMakeRect(0, 0, inner, CGFLOAT_MAX)];
      CGFloat height = MAX(19, ceil(size.height));
      caption.frame = NSMakeRect(inset, y, inner, height);
      y += height + 1;
    }
    NSControl *control = _controls[key];
    CGFloat height = disclosure ? 32 : heading ? 20 : 26;
    if ([control isKindOfClass:MTSpeedOptions.class]) {
      height = [(MTSpeedOptions *)control heightForWidth:inner];
    } else if (control.cell.wraps) {
      NSCell *cell = control.cell;
      CGFloat textWidth = disclosure ? MAX(1, inner - 40) : inner;
      height = MAX(height,
          [cell cellSizeForBounds:NSMakeRect(0, 0, textWidth, CGFLOAT_MAX)].height);
    }
    control.frame = NSMakeRect(inset, y, inner, height);
    if ([kind isEqual:@"segments"]) {
      NSControl *menu = _controls[[key stringByAppendingString:@"-menu"]];
      BOOL narrow = inner < control.intrinsicContentSize.width;
      NSControl *visible = narrow ? menu : control;
      NSControl *hidden = narrow ? control : menu;
      BOOL restoreFocus = self.window.firstResponder == hidden;
      control.hidden = narrow;
      menu.hidden = !narrow;
      menu.frame = control.frame;
      if (restoreFocus) [self.window makeFirstResponder:visible];
    }
    y += height + (_compact ? 4 : 6);
  }
  _document.frame = NSMakeRect(0, 0, _scroll.contentSize.width,
                               MAX(y, _scroll.contentSize.height));
}
@end

void *mytube_form_create(NSView *parent) {
  MTPlayerForm *form = [[MTPlayerForm alloc] initWithFrame:parent.bounds];
  [parent addSubview:form];
  return (__bridge_retained void *)form;
}
@interface MTSettingsWindow : NSWindow
@end
@implementation MTSettingsWindow
- (BOOL)performKeyEquivalent:(NSEvent *)event {
  if ((event.modifierFlags & NSEventModifierFlagCommand) &&
      ([event.charactersIgnoringModifiers isEqual:@"w"] ||
       [event.charactersIgnoringModifiers isEqual:@","])) {
    [self orderOut:nil];
    return YES;
  }
  return [super performKeyEquivalent:event];
}
- (void)cancelOperation:(id)sender { [self orderOut:nil]; }
@end

NSWindow *mytube_settings_window(NSWindow *parent) NS_RETURNS_RETAINED;
NSWindow *mytube_settings_window(NSWindow *parent) {
  NSRect frame = parent.frame;
  NSRect bounds = NSMakeRect(NSMidX(frame) - 270, NSMidY(frame) - 310, 540, 620);
  NSWindow *panel = [[MTSettingsWindow alloc] initWithContentRect:bounds
      styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskResizable | NSWindowStyleMaskFullSizeContentView
      backing:NSBackingStoreBuffered defer:NO];
  panel.releasedWhenClosed = NO;
  panel.title = @"Player settings and tools";
  panel.titleVisibility = NSWindowTitleHidden;
  panel.titlebarAppearsTransparent = YES;
  panel.titlebarSeparatorStyle = NSTitlebarSeparatorStyleNone;
  for (NSNumber *button in @[@(NSWindowCloseButton), @(NSWindowMiniaturizeButton), @(NSWindowZoomButton)])
    [panel standardWindowButton:button.integerValue].hidden = YES;
  panel.movableByWindowBackground = YES;
  panel.minSize = NSMakeSize(360, 300);
  [panel setFrame:[panel constrainFrameRect:panel.frame toScreen:parent.screen] display:NO];
  return panel;
}

// The native sidebar sits beside the OpenGL view in the accessibility tree.
void mytube_form_accessibility(MTPlayerForm *form, bool mounted) {
  NSWindow *window = form.window;
  if (!window)
    return;
  if (mounted) {
    form.accessibilityElement = YES;
    form.accessibilityRole = NSAccessibilityGroupRole;
    form.accessibilityLabel = @"Player settings";
    NSArray *children = window.accessibilityChildren ?: @[];
    form.previousWindowChildren = children;
    if (![children containsObject:form])
      window.accessibilityChildren = [children arrayByAddingObject:form];
  } else {
    window.accessibilityChildren = form.previousWindowChildren;
    form.previousWindowChildren = nil;
  }
  NSAccessibilityPostNotification(window, NSAccessibilityLayoutChangedNotification);
}

void mytube_settings_surface_style(NSView *surface, CGFloat radius) {
  if (!surface.wantsLayer) surface.wantsLayer = YES;
  CALayer *layer = surface.layer;
  if (layer.cornerRadius != radius || !layer.masksToBounds) {
    layer.cornerRadius = radius;
    layer.cornerCurve = kCACornerCurveContinuous;
    layer.masksToBounds = YES;
  }
}

void mytube_form_background(NSView *form, NSColor *color) {
  form.wantsLayer = YES;
  form.layer.backgroundColor = color.CGColor;
}
void mytube_form_update(MTPlayerForm *form, const uint8_t *bytes,
                        size_t length) {
  NSData *data = [NSData dataWithBytesNoCopy:(void *)bytes
                                      length:length
                                freeWhenDone:NO];
  NSDictionary *model = [NSJSONSerialization JSONObjectWithData:data
                                                        options:0
                                                          error:nil];
  if (model)
    [form updateForm:model];
}
NSString *mytube_form_event(MTPlayerForm *form) {
  NSArray *event = form.events.firstObject;
  if (!event)
    return nil;
  [form.events removeObjectAtIndex:0];
  NSData *data = [NSJSONSerialization dataWithJSONObject:event
                                                 options:0
                                                   error:nil];
  return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}
