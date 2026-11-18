//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.


import { Button } from '@/components/ui/button';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { ScrollArea } from '@/components/ui/scroll-area';
import { cn } from '@/lib/utils';
import { useVirtualizer } from '@tanstack/react-virtual';
import { CheckIcon, ChevronsUpDown } from 'lucide-react';
import * as React from 'react';

type Option = {
  value: string;
  label: string;
  description?: string;
};

interface VirtualizedCommandProps {
  height: string;
  options: Option[];
  placeholder: string;
  /** Search-box placeholder; defaults to `placeholder` (the trigger text). */
  searchPlaceholder?: string;
  /** Row height in px. Two-line rows (label + description) need ~48. */
  itemHeight: number;
  /** Render the list inside the styled ScrollArea instead of a native
   *  overflow container. Its viewport is still what the virtualizer drives. */
  scrollArea?: boolean;
  inputClassName?: string;
  itemClassName?: string;
  selectedOptions: string[];
  onSelectOption?: (options: string[]) => void;
  noItemsComponent?: React.ReactNode;
  multiple: boolean;
}

const VirtualizedCommand = ({
  height,
  options,
  placeholder,
  searchPlaceholder = placeholder,
  itemHeight,
  scrollArea = false,
  inputClassName,
  itemClassName,
  selectedOptions,
  onSelectOption,
  noItemsComponent = <div>No items available</div>,
  multiple,
}: VirtualizedCommandProps) => {
  const [filteredOptions, setFilteredOptions] = React.useState<Option[]>(options);
  const [focusedIndex, setFocusedIndex] = React.useState(0);
  const [isKeyboardNavActive, setIsKeyboardNavActive] = React.useState(false);

  // Native mode scrolls the cmdk list itself; ScrollArea mode scrolls its
  // viewport, and the list only carries the virtual spacer's height.
  const parentRef = React.useRef<HTMLDivElement>(null);
  const viewportRef = React.useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: filteredOptions.length,
    getScrollElement: () => (scrollArea ? viewportRef.current : parentRef.current),
    estimateSize: () => itemHeight,
  });

  const virtualOptions = virtualizer.getVirtualItems();

  const scrollToIndex = (index: number) => {
    virtualizer.scrollToIndex(index, { align: 'center' });
  };

  const handleSearch = (search: string) => {
    setIsKeyboardNavActive(false);
    setFilteredOptions(
      options.filter((option) =>
        option.value.toLowerCase().includes(search.toLowerCase()) ||
        option.label.toLowerCase().includes(search.toLowerCase()) ||
        (option.description ?? '').toLowerCase().includes(search.toLowerCase())
      ),
    );
  };

  const handleSelect = (value: string) => {
    let newSelectedOptions: string[];
    if (multiple) {
      newSelectedOptions = selectedOptions.includes(value)
        ? selectedOptions.filter((v) => v !== value)
        : [...selectedOptions, value];
    } else {
      newSelectedOptions = [value];
    }
    onSelectOption?.(newSelectedOptions);
  };

  const handleKeyDown = (event: React.KeyboardEvent) => {
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        setIsKeyboardNavActive(true);
        setFocusedIndex((prev) => {
          const newIndex = prev === -1 ? 0 : Math.min(prev + 1, filteredOptions.length - 1);
          scrollToIndex(newIndex);
          return newIndex;
        });
        break;
      case 'ArrowUp':
        event.preventDefault();
        setIsKeyboardNavActive(true);
        setFocusedIndex((prev) => {
          const newIndex = prev === -1 ? filteredOptions.length - 1 : Math.max(prev - 1, 0);
          scrollToIndex(newIndex);
          return newIndex;
        });
        break;
      case 'Enter':
        event.preventDefault();
        if (filteredOptions[focusedIndex]) {
          handleSelect(filteredOptions[focusedIndex].value);
          if (!multiple) {
            // Close the popover if not in multiple selection mode
            const popoverTrigger = document.activeElement?.closest('[role="combobox"]');
            if (popoverTrigger) {
              (popoverTrigger as HTMLElement).click();
            }
          }
        }
        break;
      default:
        break;
    }
  };

  React.useEffect(() => {
    setFilteredOptions(options);
  }, [options]);

  const listContent = (
    <>
      <CommandEmpty>{noItemsComponent}</CommandEmpty>
      <CommandGroup>
        <div
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            width: '100%',
            position: 'relative',
          }}
        >
          {virtualOptions.map((virtualOption) => {
            const option = filteredOptions[virtualOption.index];
            const isSelected = selectedOptions.includes(option.value);
            return (
              <CommandItem
                key={option.value}
                value={option.value}
                disabled={isKeyboardNavActive}
                className={cn(
                  'absolute left-0 top-0 w-full bg-transparent',
                  focusedIndex === virtualOption.index && 'bg-accent text-accent-foreground',
                  isKeyboardNavActive &&
                  focusedIndex !== virtualOption.index &&
                  'aria-selected:bg-transparent aria-selected:text-primary',
                  itemClassName,
                )}
                style={{
                  height: `${virtualOption.size}px`,
                  transform: `translateY(${virtualOption.start}px)`,
                }}
                onMouseEnter={() => !isKeyboardNavActive && setFocusedIndex(virtualOption.index)}
                onMouseLeave={() => !isKeyboardNavActive && setFocusedIndex(-1)}
                onSelect={() => handleSelect(option.value)}
              >
                {multiple && (
                  <div
                    className={cn(
                      'mr-2 flex h-4 w-4 items-center justify-center rounded-sm border border-primary',
                      isSelected ? 'bg-primary text-primary-foreground' : 'opacity-50 [&_svg]:invisible',
                    )}
                  >
                    <CheckIcon className="h-4 w-4" />
                  </div>
                )}
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate">{option.label}</span>
                  {option.description && (
                    <span className="truncate text-xs text-gray-500">
                      {option.description}
                    </span>
                  )}
                </div>
              </CommandItem>
            );
          })}
        </div>
      </CommandGroup>
    </>
  );

  return (
    <Command shouldFilter={false} onKeyDown={handleKeyDown}>
      <CommandInput
        className={inputClassName}
        onValueChange={handleSearch}
        placeholder={searchPlaceholder}
      />
      {scrollArea ? (
        <ScrollArea style={{ height, width: '100%' }} viewportRef={viewportRef}>
          <CommandList
            className='!max-h-none !overflow-visible'
            onMouseDown={() => setIsKeyboardNavActive(false)}
            onMouseMove={() => setIsKeyboardNavActive(false)}
          >
            {listContent}
          </CommandList>
        </ScrollArea>
      ) : (
        <CommandList
          ref={parentRef}
          style={{
            height: height,
            width: '100%',
            overflow: 'auto',
          }}
          onMouseDown={() => setIsKeyboardNavActive(false)}
          onMouseMove={() => setIsKeyboardNavActive(false)}
        >
          {listContent}
        </CommandList>
      )}
    </Command>
  );
};

interface VirtualizedSelectProps {
  options: Option[];
  placeholder?: string;
  /** Search-box placeholder; defaults to `placeholder` (the trigger text). */
  searchPlaceholder?: string;
  height?: string;
  /** Row height in px. Two-line rows (label + description) need ~48. */
  itemHeight?: number;
  /** Use the styled ScrollArea for the list (virtualization still applies). */
  scrollArea?: boolean;
  inputClassName?: string;
  itemClassName?: string;
  className?: string;
  isLoading: boolean;
  disabled?: boolean;
  onSelectOption?: (options: string[]) => void;
  value?: string | string[];
  defaultValue?: string | string[];
  noItemsComponent?: React.ReactNode;
  multiple?: boolean;
  size?: 'default' | 'sm' | 'lg' | 'icon';
}

export function VirtualizedSelect({
  options,
  onSelectOption,
  className,
  defaultValue,
  value,
  size = 'default',
  isLoading,
  disabled = false,
  placeholder = 'Search items...',
  searchPlaceholder,
  height = '300px',
  itemHeight = 35,
  scrollArea = false,
  inputClassName,
  itemClassName,
  noItemsComponent,
  multiple = false,
}: VirtualizedSelectProps) {
  const [open, setOpen] = React.useState(false);
  const [selectedOptions, setSelectedOptions] = React.useState<string[]>(
    value !== undefined
      ? Array.isArray(value)
        ? value
        : value ? [value] : []
      : defaultValue !== undefined
        ? Array.isArray(defaultValue)
          ? defaultValue
          : defaultValue ? [defaultValue] : []
        : []
  );

  React.useEffect(() => {
    if (value !== undefined) {
      setSelectedOptions(Array.isArray(value) ? value : value ? [value] : []);
    }
  }, [value]);

  const getDisplayText = () => {
    if (isLoading) return 'Loading...';

    if (selectedOptions.length === 0) return placeholder;

    if (!multiple) {
      const selectedItem = options.find(option => option.value === selectedOptions[0]);
      return selectedItem?.label || placeholder;
    }

    const selectedLabels = selectedOptions
      .map(value => options.find(option => option.value === value)?.label)
      .filter(Boolean);

    if (selectedLabels.length === 0) return placeholder;
    if (selectedLabels.length > 2) {
      return `${selectedLabels.slice(0, 2).join(', ')}, +${selectedLabels.length - 2}`;
    }
    return selectedLabels.join(", ");
  };

  return (
    <div className="w-full">
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            size={size}
            variant="outline"
            role="combobox"
            aria-expanded={open}
            className={cn('justify-between', className)}
            disabled={isLoading || disabled}
          >
            <span className='truncate'>{getDisplayText()}</span>
            <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
          </Button>
        </PopoverTrigger>
        {!isLoading && (
          <PopoverContent className="p-0" align="start" side="bottom">
            <VirtualizedCommand
              height={height}
              options={options}
              placeholder={placeholder}
              searchPlaceholder={searchPlaceholder}
              itemHeight={itemHeight}
              scrollArea={scrollArea}
              inputClassName={inputClassName}
              itemClassName={itemClassName}
              selectedOptions={selectedOptions}
              onSelectOption={(newSelectedOptions) => {
                setSelectedOptions(newSelectedOptions);
                onSelectOption?.(newSelectedOptions);
                if (!multiple) setOpen(false);
              }}
              noItemsComponent={noItemsComponent}
              multiple={multiple}
            />
          </PopoverContent>
        )}
      </Popover>
    </div>
  );
}
