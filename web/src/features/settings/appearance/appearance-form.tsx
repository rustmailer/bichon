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
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { useTheme } from '@/context/theme-context'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { useTranslation } from 'react-i18next'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { update_user } from '@/api/users/api'
import { toast } from '@/hooks/use-toast'
import { AxiosError } from 'axios'


const languages = [
    { value: 'ar', label: 'العربية' },
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
    theme: z.enum(['light', 'dark'], {
        required_error: t('settings.appearance.validation.theme.required'),
    }),
    language: z.string({
        required_error: t('settings.appearance.validation.language.required'),
    })
})

type AppearanceFormValues = z.infer<ReturnType<typeof appearanceSchema>>


export function AppearanceForm() {
    const { data: user } = useCurrentUser();
    const queryClient = useQueryClient();
    const { t, i18n } = useTranslation();
    const { theme, setTheme } = useTheme();


    const form = useForm<AppearanceFormValues>({
        resolver: zodResolver(appearanceSchema(t)),
        mode: 'onChange',
        defaultValues: {
            theme: (theme as 'light' | 'dark') || 'light',
            language: i18n.language || 'en',
        },
    })

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
    });

    function onSubmit(data: AppearanceFormValues) {
        i18n.changeLanguage(data.language);
        setTheme(data.theme);
        mutation.mutate(data);
    }

    return (
        <div className="w-full max-w-6xl ml-0 px-4">
            <Form {...form}>
                <form
                    onSubmit={form.handleSubmit(onSubmit)}
                    className="space-y-8 w-full max-w-screen-xl mx-auto px-4 md:px-6"
                >
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
                        render={({ field }) => (
                            <FormItem className="space-y-1">
                                <FormLabel>{t('settings.appearance.field.theme')}</FormLabel>
                                <FormDescription>
                                    {t('settings.appearance.description.theme')}
                                </FormDescription>
                                <FormMessage />
                                <RadioGroup
                                    onValueChange={field.onChange}
                                    defaultValue={field.value}
                                    className='grid max-w-md grid-cols-2 gap-8 pt-2'
                                >
                                    <FormItem>
                                        <FormLabel className='[&:has([data-state=checked])>div]:border-primary cursor-pointer'>
                                            <FormControl>
                                                <RadioGroupItem value='light' className='sr-only' />
                                            </FormControl>
                                            <div className='items-center rounded-md border-2 border-muted p-1 hover:border-accent'>
                                                <div className='space-y-2 rounded-sm bg-[#ecedef] p-2'>
                                                    <div className='space-y-2 rounded-md bg-white p-2 shadow-sm'>
                                                        <div className='h-2 w-[80px] rounded-lg bg-[#ecedef]' />
                                                        <div className='h-2 w-[100px] rounded-lg bg-[#ecedef]' />
                                                    </div>
                                                    <div className='flex items-center space-x-2 rounded-md bg-white p-2 shadow-sm'>
                                                        <div className='h-4 w-4 rounded-full bg-[#ecedef]' />
                                                        <div className='h-2 w-[100px] rounded-lg bg-[#ecedef]' />
                                                    </div>
                                                </div>
                                            </div>
                                            <span className='block w-full p-2 text-center font-normal'>
                                                {t('settings.appearance.theme.light')}
                                            </span>
                                        </FormLabel>
                                    </FormItem>
                                    <FormItem>
                                        <FormLabel className='[&:has([data-state=checked])>div]:border-primary cursor-pointer'>
                                            <FormControl>
                                                <RadioGroupItem value='dark' className='sr-only' />
                                            </FormControl>
                                            <div className='items-center rounded-md border-2 border-muted bg-popover p-1 hover:bg-accent hover:text-accent-foreground'>
                                                <div className='space-y-2 rounded-sm bg-slate-950 p-2'>
                                                    <div className='space-y-2 rounded-md bg-slate-800 p-2 shadow-sm'>
                                                        <div className='h-2 w-[80px] rounded-lg bg-slate-400' />
                                                        <div className='h-2 w-[100px] rounded-lg bg-slate-400' />
                                                    </div>
                                                    <div className='flex items-center space-x-2 rounded-md bg-slate-800 p-2 shadow-sm'>
                                                        <div className='h-4 w-4 rounded-full bg-slate-400' />
                                                        <div className='h-2 w-[100px] rounded-lg bg-slate-400' />
                                                    </div>
                                                </div>
                                            </div>
                                            <span className='block w-full p-2 text-center font-normal'>
                                                {t('settings.appearance.theme.dark')}
                                            </span>
                                        </FormLabel>
                                    </FormItem>
                                </RadioGroup>
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