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

import { useEffect } from 'react'
import { z } from 'zod'
import { useForm } from 'react-hook-form'
import { CaretSortIcon, CheckIcon } from '@radix-ui/react-icons'
import { zodResolver } from '@hookform/resolvers/zod'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import {
    Form,
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from '@/components/ui/form'
import { RadioGroup } from '@/components/ui/radio-group'
import { useTheme, Theme } from '@/context/theme-context'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { useTranslation } from 'react-i18next'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { update_user } from '@/api/users/api'
import { toast } from '@/hooks/use-toast'
import { AxiosError } from 'axios'
import { Card } from '@/components/ui/card'
import { Switch } from '@/components/ui/switch'
import {
    THEME_FAMILIES,
    THEME_MODES,
    type ThemeMode,
    type ThemeSwatch,
    isTheme,
    themeFamily,
    themeMode,
} from '@/lib/theme'
import { SplitThemePreview, ThemePreview } from '@/components/theme-preview'

const languages = [
    { value: 'ar', label: 'العربية' },
    { value: 'cs', label: 'Čeština' },
    { value: 'da', label: 'Dansk' },
    { value: 'de', label: 'Deutsch' },
    { value: 'en', label: 'English' },
    { value: 'es', label: 'Español' },
    { value: 'fi', label: 'Suomi' },
    { value: 'fr', label: 'Français' },
    { value: 'it', label: 'Italiano' },
    { value: 'jp', label: '日本語' },
    { value: 'ko', label: '한국어' },
    { value: 'nl', label: 'Nederlands' },
    { value: 'no', label: 'Norsk' },
    { value: 'pl', label: 'Polski' },
    { value: 'pt', label: 'Português' },
    { value: 'ru', label: 'Русский' },
    { value: 'sv', label: 'Svenska' },
    { value: 'zh', label: '中文' },
    { value: 'zh-tw', label: '繁體中文' },
]

const appearanceSchema = (t: (key: string) => string) => z.object({
    theme: z.custom<Theme>(isTheme, {
        message: t('settings.appearance.validation.theme.required'),
    }),
    language: z.string({
        required_error: t('settings.appearance.validation.language.required'),
    })
})

type AppearanceFormValues = z.infer<ReturnType<typeof appearanceSchema>>

type ThemeCard = {
    key: string
    /** The theme applied when the card is picked. */
    value: Theme
    labelKey: string
    swatch: ThemeSwatch
    /** Set for family cards: `swatch` is the light variant, previewed beside this one. */
    dark?: ThemeSwatch
}

function themeCards(followSystem: boolean, shownMode: ThemeMode): ThemeCard[] {
    if (followSystem) {
        // One card per family while following the system. Picking it keeps
        // the variant currently shown, so turning the toggle off is seamless.
        return THEME_FAMILIES.map((family) => ({
            key: family.id,
            value: family[shownMode].value,
            labelKey: family.labelKey,
            swatch: family.light,
            dark: family.dark,
        }))
    }
    // Every light variant, then every dark one.
    return THEME_MODES.flatMap((mode) =>
        THEME_FAMILIES.map((family) => ({
            key: family[mode].value,
            value: family[mode].value,
            labelKey: family.labelKey,
            swatch: family[mode],
        }))
    )
}

export function AppearanceForm() {
    const { data: user } = useCurrentUser()
    const queryClient = useQueryClient()
    const { t, i18n } = useTranslation()
    const { theme, setTheme, followSystem, setFollowSystem, resolvedTheme } = useTheme()

    const form = useForm<AppearanceFormValues>({
        resolver: zodResolver(appearanceSchema(t)),
        mode: 'onChange',
        defaultValues: {
            theme,
            language: i18n.language || 'en',
        },
    })

    // The theme applies on pick, from here or the header; keep the field on it.
    useEffect(() => {
        form.setValue('theme', theme)
    }, [theme, form])

    const mutation = useMutation({
        mutationFn: async (values: AppearanceFormValues) => {
            return update_user(user!.id, values)
        },
        onSuccess: () => {
            queryClient.invalidateQueries({ queryKey: ['current-user'] })
            toast({ title: t('settings.profile.toast.updated') })
        },
        onError: (err: AxiosError) => {
            toast({
                variant: 'destructive',
                title: t('settings.profile.toast.update_failed'),
                description: (err.response?.data as any)?.message || err.message,
            })
        },
    })

    function onSubmit(data: AppearanceFormValues) {
        i18n.changeLanguage(data.language)
        mutation.mutate(data)
    }

    return (
        <div className="w-full max-w-6xl ml-0 px-4">
            <Form {...form}>
                <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-10 w-full max-w-screen-xl mx-auto px-4 md:px-6">
                    <FormField
                        control={form.control}
                        name='language'
                        render={({ field }) => (
                            <FormItem className='flex flex-col'>
                                <FormLabel>{t('settings.appearance.field.language')}</FormLabel>
                                <Popover>
                                    <PopoverTrigger asChild>
                                        <FormControl>
                                            <Button
                                                variant='outline'
                                                role='combobox'
                                                className={cn(
                                                    'w-[400px] justify-between',
                                                    !field.value && 'text-muted-foreground'
                                                )}
                                            >
                                                {field.value
                                                    ? languages.find((l) => l.value === field.value)?.label
                                                    : t('settings.appearance.placeholder.select_language')}
                                                <CaretSortIcon className='ms-2 h-4 w-4 shrink-0 opacity-50' />
                                            </Button>
                                        </FormControl>
                                    </PopoverTrigger>
                                    <PopoverContent className='w-[400px] p-0' align="start">
                                        <Command>
                                            <CommandInput placeholder={t('settings.appearance.command.search')} />
                                            <CommandEmpty>{t('settings.appearance.command.no_results')}</CommandEmpty>
                                            <CommandList>
                                                <CommandGroup>
                                                    {languages.map((language) => (
                                                        <CommandItem
                                                            value={language.label}
                                                            key={language.value}
                                                            onSelect={() => {
                                                                form.setValue('language', language.value)
                                                            }}
                                                        >
                                                            <CheckIcon
                                                                className={cn(
                                                                    'mr-2 h-4 w-4',
                                                                    language.value === field.value ? 'opacity-100' : 'opacity-0'
                                                                )}
                                                            />
                                                            {language.label}
                                                        </CommandItem>
                                                    ))}
                                                </CommandGroup>
                                            </CommandList>
                                        </Command>
                                    </PopoverContent>
                                </Popover>
                                <FormDescription>
                                    {t('settings.appearance.description.language')}
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />

                    <FormField
                        control={form.control}
                        name='theme'
                        render={() => (
                            <FormItem className="space-y-4">
                                <div>
                                    <FormLabel className="text-base">{t('settings.appearance.field.theme')}</FormLabel>
                                    <FormDescription>{t('settings.appearance.description.theme')}</FormDescription>
                                </div>
                                <div className='flex items-center justify-between gap-4 rounded-lg border p-4 max-w-xl'>
                                    <div className='space-y-0.5'>
                                        <label htmlFor='appearance-follow-system' className='text-sm font-medium'>
                                            {t('theme.use_system')}
                                        </label>
                                        <p className='text-sm text-muted-foreground'>
                                            {t('settings.appearance.description.use_system')}
                                        </p>
                                    </div>
                                    <Switch
                                        id='appearance-follow-system'
                                        checked={followSystem}
                                        onCheckedChange={setFollowSystem}
                                    />
                                </div>
                                <RadioGroup
                                    className='grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-4 pt-2'
                                >
                                    {themeCards(followSystem, themeMode(resolvedTheme)).map((item) => {
                                        // Same rule as the header's theme switch.
                                        const selected = followSystem
                                            ? themeFamily(item.value) === themeFamily(resolvedTheme)
                                            : item.value === resolvedTheme

                                        return (
                                            <Card
                                                key={item.key}
                                                onClick={() => setTheme(item.value)}
                                                className={cn(
                                                    "cursor-pointer overflow-hidden border-2 transition-all hover:shadow-md",
                                                    selected ? "border-primary shadow-md" : "border-muted opacity-80"
                                                )}
                                            >
                                                <div className="p-2">
                                                    {item.dark ? (
                                                        <SplitThemePreview light={item.swatch} dark={item.dark} size='lg' />
                                                    ) : (
                                                        <ThemePreview swatch={item.swatch} size='lg' />
                                                    )}

                                                    <div className="mt-2 flex items-center justify-between">
                                                        <span className="text-[11px] font-medium uppercase truncate">
                                                            {t(item.labelKey)}
                                                        </span>

                                                        {selected && (
                                                            <CheckIcon className="h-3.5 w-3.5 text-primary" />
                                                        )}
                                                    </div>
                                                </div>
                                            </Card>
                                        )
                                    })}
                                </RadioGroup>
                                <FormMessage />
                            </FormItem>
                        )}
                    />

                    <div className="flex justify-start pt-4">
                        <Button type='submit'>
                            {t('settings.appearance.button.update')}
                        </Button>
                    </div>
                </form>
            </Form>
        </div>
    )
}